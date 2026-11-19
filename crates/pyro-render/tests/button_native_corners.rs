//! A button of native appearance has Chromium's native look's 1px corners, which are not its
//! computed `border-radius`: a button an author styles the border or background of (its appearance
//! devolves) and one of `appearance: none` are square alike (css-ui-4's appearance-disabling
//! properties; WPT css-ui `compute-kind-widget-generated/*`).
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// The red channel of the button's top-left corner pixel (it is absolutely positioned at 10,10).
fn corner(style: &str) -> u8 {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0}} button{{position:absolute;left:10px;top:10px;{style}}}</style><button>button</button>"),
        base_url: "http://test/".into(),
        width: 200,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0.data[(10 * 200 + 10) * 4]
}

#[test]
fn a_native_button_is_rounded_and_a_primitive_one_square() {
    let native = corner("");
    let square = corner("appearance:none");
    // The border is #999 (153): a rounded corner shows some of the white page through it.
    assert_eq!(square, 153);
    assert!(native > 153, "native corner {native}");
    assert_eq!(corner("background-color:#efefef"), square, "author background");
    assert_eq!(corner("border-color:#999"), square, "author border");
    assert_eq!(corner("appearance:checkbox"), native, "a compat keyword acts as auto");
    assert_eq!(corner("appearance:textfield"), native, "textfield acts as auto on a button");
    assert!(corner("background-color:#efefef;border-radius:1px") > 153, "an author radius rounds it");
}
