// Trusted Types (https://w3c.github.io/trusted-types/dist/spec/), made on a realm's first use
// (trusted_types.rs): TrustedHTML, TrustedScript, TrustedScriptURL, TrustedTypePolicy,
// TrustedTypePolicyFactory and the realm's `trustedTypes` factory. `native` reports what the
// realm's CSP allows for policy names (`trusted-types` directive); with no such directive every
// name is allowed, more than once.
(native => {
  'use strict';
  const freeze = Object.freeze;
  const defineProperty = Object.defineProperty;
  const apply = Reflect.apply;
  const String_ = String;
  const illegal = () => new TypeError('Illegal constructor');
  const ILLEGAL = Symbol('illegal');

  // [[Data]] of each Trusted* object, by kind.
  const data = new WeakMap();
  function makeTrusted(Ctor, value) {
    const object = Reflect.construct(Ctor, [ILLEGAL], Ctor);
    data.set(object, { kind: Ctor, value });
    return object;
  }
  function dataOf(Ctor, self, what) {
    const d = data.get(self);
    if (d === undefined || d.kind !== Ctor) throw new TypeError(`Failed to execute '${what}' on '${Ctor.name}': Illegal invocation`);
    return d.value;
  }
  function trustedClass(name) {
    const Ctor = {
      [name]: class {
        constructor(token) { if (token !== ILLEGAL) throw illegal(); }
        toString() { return dataOf(Ctor, this, 'toString'); }
        toJSON() { return dataOf(Ctor, this, 'toJSON'); }
      },
    }[name];
    defineProperty(Ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
    return Ctor;
  }
  const TrustedHTML = trustedClass('TrustedHTML');
  const TrustedScript = trustedClass('TrustedScript');
  const TrustedScriptURL = trustedClass('TrustedScriptURL');
  for (const [Ctor, method] of [[TrustedHTML, 'fromLiteral'], [TrustedScript, 'fromLiteral'], [TrustedScriptURL, 'fromLiteral']]) {
    // Trusted Types: `fromLiteral(templateStringsArray)` (a template literal without substitutions).
    defineProperty(Ctor, method, {
      value: function fromLiteral(templateStringsArray) {
        if (typeof templateStringsArray !== 'object' || templateStringsArray === null || !Object.isFrozen(templateStringsArray)
            || !Array.isArray(templateStringsArray.raw) || templateStringsArray.length !== 1) {
          throw new TypeError(`Failed to execute 'fromLiteral' on '${Ctor.name}': the argument is not a template literal without substitutions.`);
        }
        return makeTrusted(Ctor, String_(templateStringsArray[0]));
      },
      writable: true, configurable: true,
    });
  }

  // Each policy: its name and options.
  const policies = new WeakMap();
  const TrustedTypePolicy = class TrustedTypePolicy {
    constructor(token) { if (token !== ILLEGAL) throw illegal(); }
    get name() { return policyOf(this, 'name').name; }
    createHTML(input, ...args) { return create(policyOf(this, 'createHTML'), TrustedHTML, 'createHTML', input, args); }
    createScript(input, ...args) { return create(policyOf(this, 'createScript'), TrustedScript, 'createScript', input, args); }
    createScriptURL(input, ...args) { return create(policyOf(this, 'createScriptURL'), TrustedScriptURL, 'createScriptURL', input, args); }
  };
  defineProperty(TrustedTypePolicy.prototype, Symbol.toStringTag, { value: 'TrustedTypePolicy', configurable: true });
  function policyOf(self, what) {
    const p = policies.get(self);
    if (p === undefined) throw new TypeError(`Failed to execute '${what}' on 'TrustedTypePolicy': Illegal invocation`);
    return p;
  }
  // "Get Trusted Type policy value" and "create a Trusted Type".
  function policyValue(policy, method, input, args, throwIfMissing) {
    const fn = policy.options[method];
    if (fn === undefined || fn === null) {
      if (throwIfMissing) throw new TypeError(`Failed to execute '${method}' on 'TrustedTypePolicy': Policy ${policy.name}'s TrustedTypePolicyOptions did not specify a '${method}' member.`);
      return null;
    }
    return apply(fn, undefined, [String_(input), ...args]);
  }
  function create(policy, Ctor, method, input, args) {
    const value = policyValue(policy, method, input, args, true);
    return makeTrusted(Ctor, value === null || value === undefined ? '' : String_(value));
  }

  // TrustedTypePolicyOptions: callbacks read once, at creation.
  function options(value) {
    if (value === undefined || value === null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeError("Failed to execute 'createPolicy' on 'TrustedTypePolicyFactory': The provided value is not of type 'TrustedTypePolicyOptions'.");
    }
    const out = {};
    for (const key of ['createHTML', 'createScript', 'createScriptURL']) {
      const fn = value[key];
      if (fn !== undefined && fn !== null && typeof fn !== 'function') {
        throw new TypeError(`Failed to execute 'createPolicy' on 'TrustedTypePolicyFactory': The '${key}' property is not a function.`);
      }
      out[key] = fn ?? undefined;
    }
    return out;
  }

  const created = [];
  let defaultPolicy = null;
  let emptyHTML = null;
  let emptyScript = null;
  const TrustedTypePolicyFactory = class TrustedTypePolicyFactory {
    constructor(token) { if (token !== ILLEGAL) throw illegal(); }
    createPolicy(policyName, policyOptions) {
      self_(this, 'createPolicy');
      if (arguments.length < 1) throw new TypeError("Failed to execute 'createPolicy' on 'TrustedTypePolicyFactory': 1 argument required, but only 0 present.");
      const name = String_(policyName);
      // CSP "Should Trusted Type policy creation be blocked by Content Security Policy?"
      const blocked = native.policyBlocked(name, created.includes(name));
      if (blocked) {
        throw new TypeError(`Failed to execute 'createPolicy' on 'TrustedTypePolicyFactory': Policy "${name}" disallowed.`);
      }
      const policy = Reflect.construct(TrustedTypePolicy, [ILLEGAL], TrustedTypePolicy);
      policies.set(policy, { name, options: options(policyOptions) });
      created.push(name);
      if (name === 'default') defaultPolicy = policy;
      return policy;
    }
    isHTML(value) { self_(this, 'isHTML'); return data.get(value)?.kind === TrustedHTML; }
    isScript(value) { self_(this, 'isScript'); return data.get(value)?.kind === TrustedScript; }
    isScriptURL(value) { self_(this, 'isScriptURL'); return data.get(value)?.kind === TrustedScriptURL; }
    get emptyHTML() { self_(this, 'emptyHTML'); return emptyHTML ??= freeze(makeTrusted(TrustedHTML, '')); }
    get emptyScript() { self_(this, 'emptyScript'); return emptyScript ??= freeze(makeTrusted(TrustedScript, '')); }
    get defaultPolicy() { self_(this, 'defaultPolicy'); return defaultPolicy; }
    getAttributeType(tagName, attribute, elementNs = '', attrNs = '') {
      self_(this, 'getAttributeType');
      return attributeType(String_(tagName).toLowerCase(), String_(attribute).toLowerCase(), nsOf(elementNs), nsOf(attrNs));
    }
    getPropertyType(tagName, property, elementNs = '') {
      self_(this, 'getPropertyType');
      return propertyType(String_(tagName).toLowerCase(), String_(property), nsOf(elementNs));
    }
  };
  defineProperty(TrustedTypePolicyFactory.prototype, Symbol.toStringTag, { value: 'TrustedTypePolicyFactory', configurable: true });
  const factory = Reflect.construct(TrustedTypePolicyFactory, [ILLEGAL], TrustedTypePolicyFactory);
  function self_(value, what) {
    if (value !== factory) throw new TypeError(`Failed to execute '${what}' on 'TrustedTypePolicyFactory': Illegal invocation`);
  }

  const HTML_NS = 'http://www.w3.org/1999/xhtml';
  const SVG_NS = 'http://www.w3.org/2000/svg';
  const XLINK_NS = 'http://www.w3.org/1999/xlink';
  // An empty or null namespace is the HTML namespace for elements (the spec's default).
  const nsOf = ns => (ns === undefined || ns === null || ns === '' ? '' : String_(ns));
  // "Get Trusted Type data for attribute": event handler content attributes are TrustedScript;
  // the table: iframe srcdoc (TrustedHTML), HTML script src and SVG script href (TrustedScriptURL).
  function attributeType(tag, attribute, elementNs, attrNs) {
    const elNs = elementNs === '' ? HTML_NS : elementNs;
    if (attrNs === '' && attribute.startsWith('on') && attribute.length > 2) return 'TrustedScript';
    if (elNs === HTML_NS && attrNs === '' && tag === 'iframe' && attribute === 'srcdoc') return 'TrustedHTML';
    if (elNs === HTML_NS && attrNs === '' && tag === 'script' && attribute === 'src') return 'TrustedScriptURL';
    if (elNs === SVG_NS && tag === 'script' && attribute === 'href' && (attrNs === '' || attrNs === XLINK_NS)) return 'TrustedScriptURL';
    return null;
  }
  // "Get Trusted Type data for property".
  function propertyType(tag, property, elementNs) {
    const elNs = elementNs === '' ? HTML_NS : elementNs;
    if (elNs === HTML_NS) {
      if (tag === 'iframe' && property === 'srcdoc') return 'TrustedHTML';
      if (tag === 'script' && (property === 'innerText' || property === 'textContent' || property === 'text')) return 'TrustedScript';
      if (tag === 'script' && property === 'src') return 'TrustedScriptURL';
    }
    if (property === 'innerHTML' || property === 'outerHTML') return 'TrustedHTML';
    return null;
  }

  return {
    TrustedHTML, TrustedScript, TrustedScriptURL, TrustedTypePolicy, TrustedTypePolicyFactory,
    trustedTypes: factory,
    // For native sinks (slice 2): the trusted value's text, or null.
    trustedData(kind, value) {
      const d = data.get(value);
      const Ctor = kind === 'html' ? TrustedHTML : kind === 'script' ? TrustedScript : TrustedScriptURL;
      return d !== undefined && d.kind === Ctor ? d.value : null;
    },
    // The default policy's value for a sink (null when there is none or it has no such member).
    defaultPolicyValue(method, input, sink) {
      if (defaultPolicy === null) return undefined;
      return policyValue(policies.get(defaultPolicy), method, input, [sink], false);
    },
  };
})
