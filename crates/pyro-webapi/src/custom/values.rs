//! A definition belongs to its registry. Scoped registries can become
//! unreachable while the Window lives, including cycles through a constructor,
//! a callback, or a reaction to a pending whenDefined promise.

use super::*;
use pyro_js::{Deferred, IdentityKey, WeakRealm};

const VALUES: &str = "customElementRegistryValues";

pub(super) enum RegistryValue {
    Global(Persistent),
    Scoped { realm: WeakRealm, identity: IdentityKey, slot: u32 },
}

impl RegistryValue {
    fn scoped(s: &Scope<'_>, registry: &RegistryObject, value: Value<'_>) -> JsResult<Self> {
        let identity = registry.key.identity(&registry.page);
        let realm = if s.realm_id() == registry.key.realm {
            s.realm()
        } else {
            registry
                .page
                .realms
                .borrow()
                .get(&registry.key.realm)
                .and_then(|weak| weak.upgrade())
                .ok_or(JsError::ContextDestroyed)?
        };
        let home = realm.scope()?;
        let wrapper = home.lookup_identity(identity).ok_or(JsError::ContextDestroyed)?;
        let holder = match home.get_hidden(wrapper, VALUES)?.and_then(|v| v.as_object()) {
            Some(holder) => holder,
            None => {
                let holder = home.object();
                holder.set_prototype(home.null());
                holder.set_atom("length", home.number(0.0))?;
                home.set_hidden(wrapper, VALUES, holder.value())?;
                holder
            }
        };
        let slot = holder.get_atom("length")?.to_number()? as u32;
        let next =
            slot.checked_add(1).ok_or_else(|| JsError::Range("Too many custom element registry values".into()))?;
        holder.set_index(slot, home.rehome(value))?;
        holder.set_atom("length", home.number(f64::from(next)))?;
        Ok(Self::Scoped { realm: realm.downgrade(), identity, slot })
    }

    fn holder<'s>(&self, s: &Scope<'s>) -> Option<(Object<'s>, u32)> {
        let Self::Scoped { realm, identity, slot } = self else { return None };
        let realm = realm.upgrade()?;
        let home = realm.scope().ok()?;
        let wrapper = home.lookup_identity(*identity)?;
        let holder = home.get_hidden(wrapper, VALUES).ok()??.as_object()?;
        Some((s.rehome_object(holder), *slot))
    }

    pub(super) fn get<'s>(&self, s: &Scope<'s>) -> Value<'s> {
        match self {
            Self::Global(value) => value.get(s),
            Self::Scoped { .. } => {
                self.holder(s).and_then(|(h, slot)| h.get_index(slot).ok()).unwrap_or_else(|| s.undefined())
            }
        }
    }

    fn clear(&self, s: &Scope<'_>) -> JsResult<()> {
        if let Some((holder, slot)) = self.holder(s) {
            holder.set_index(slot, s.undefined())?;
        }
        Ok(())
    }
}

impl Definition {
    /// Property access during define can reenter script or throw. Keep the
    /// temporary roots until all getters have succeeded, then transfer them
    /// to the registry wrapper before publishing the definition.
    pub(super) fn trace_scoped(&mut self, s: &Scope<'_>, registry: &RegistryObject) -> JsResult<()> {
        if registry.key.scoped.is_none() {
            return Ok(());
        }
        let callbacks = &mut self.callbacks;
        for value in [
            Some(&mut self.ctor),
            callbacks.connected.as_mut(),
            callbacks.disconnected.as_mut(),
            callbacks.connected_move.as_mut(),
            callbacks.adopted.as_mut(),
            callbacks.attribute_changed.as_mut(),
            callbacks.form_associated.as_mut(),
            callbacks.form_reset.as_mut(),
            callbacks.form_disabled.as_mut(),
            callbacks.form_state_restore.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            *value = RegistryValue::scoped(s, registry, value.get(s))?;
        }
        Ok(())
    }
}

pub(super) enum PendingDefinition {
    Global(Deferred),
    Scoped { promise: RegistryValue, resolve: RegistryValue },
}

impl PendingDefinition {
    pub(super) fn new(s: &Scope<'_>, registry: &RegistryObject, deferred: Deferred) -> JsResult<Self> {
        if registry.key.scoped.is_none() {
            return Ok(Self::Global(deferred));
        }
        let promise = RegistryValue::scoped(s, registry, deferred.promise.get(s))?;
        let resolve = RegistryValue::scoped(s, registry, deferred.resolver(s))?;
        Ok(Self::Scoped { promise, resolve })
    }

    pub(super) fn promise<'s>(&self, s: &Scope<'s>) -> Value<'s> {
        match self {
            Self::Global(d) => d.promise.get(s),
            Self::Scoped { promise, .. } => promise.get(s),
        }
    }

    pub(super) fn resolve(self, s: &Scope<'_>, ctor: Value<'_>) -> JsResult<()> {
        match self {
            Self::Global(d) => d.resolve(s, ctor),
            Self::Scoped { promise, resolve } => {
                let function = resolve.get(s).as_object().ok_or(JsError::ContextDestroyed)?;
                let result = function.call(None, &[s.rehome(ctor)]);
                resolve.clear(s)?;
                promise.clear(s)?;
                result.map(|_| ())
            }
        }
    }
}
