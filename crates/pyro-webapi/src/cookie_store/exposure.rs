//! Cookie Store interfaces are exposed in secure windows and service workers.
//! Install lazy own data properties without placing interface constructors on
//! WorkerGlobalScope.prototype or initializing the cookie service eagerly.
use pyro_js::{JsResult, LazyGlobalInterface, Scope, Value};

macro_rules! interface {
    ($name:ident) => {
        struct $name;

        impl LazyGlobalInterface for $name {
            const NAME: &'static str = stringify!($name);

            fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
                let page = crate::target::page_of(s)?;
                super::member(s, &page, Self::NAME)
            }
        }
    };
}

interface!(CookieStore);
interface!(CookieStoreManager);
interface!(CookieChangeEvent);
interface!(ExtendableCookieChangeEvent);

pub(crate) fn install(s: &Scope<'_>, service_worker: bool) -> JsResult<()> {
    s.define_lazy_global_interface::<CookieStore>()?;
    s.define_lazy_global_interface::<CookieStoreManager>()?;
    if service_worker {
        s.define_lazy_global_interface::<ExtendableCookieChangeEvent>()
    } else {
        s.define_lazy_global_interface::<CookieChangeEvent>()
    }
}
