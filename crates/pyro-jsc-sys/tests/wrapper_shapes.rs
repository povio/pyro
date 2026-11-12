//! `JSObjectMakeWrapper` (`webkit/patches/0018`) makes a wrapper only in the
//! global object that made its shape: for another one it returns NULL and the
//! caller makes the wrapper the old way. Skipped without the patched JSC.

use pyro_jsc_sys::*;
use std::ptr;

#[test]
fn wrapper_shape_serves_the_global_object_that_made_it() {
    let spi = spi::get();
    let (Some(create), Some(release), Some(make)) =
        (spi.wrapper_shape_create, spi.wrapper_shape_release, spi.object_make_wrapper)
    else {
        return;
    };
    unsafe {
        let group = JSContextGroupCreate();
        let a = JSGlobalContextCreateInGroup(group, ptr::null());
        let b = JSGlobalContextCreateInGroup(group, ptr::null());
        let class = JSClassCreate(&JSClassDefinition::EMPTY);
        let proto = JSObjectMake(a, ptr::null(), ptr::null_mut());
        let shape = create(a, class, proto);
        assert!(!shape.is_null());

        let mut weak: JSWeakRef = ptr::null();
        assert!(make(b, shape, ptr::null_mut(), ptr::null(), &mut weak).is_null(), "another global object");
        assert!(weak.is_null());

        let obj = make(a, shape, ptr::null_mut(), ptr::null(), &mut weak);
        assert!(!obj.is_null());
        assert!(weak.is_null(), "no handle without a keep-alive byte");
        assert!(JSValueIsStrictEqual(a, JSObjectGetPrototype(a, obj), proto as JSValueRef));

        release(group, shape);
        JSClassRelease(class);
        JSGlobalContextRelease(b);
        JSGlobalContextRelease(a);
        JSContextGroupRelease(group);
    }
}
