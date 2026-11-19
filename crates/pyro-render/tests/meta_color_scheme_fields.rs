//! `<meta name=color-scheme content=dark>`: default `<input>` and `<textarea>` take the dark
//! `Field` background with the light `FieldText`, as Chrome draws them (they kept a white
//! background with the inherited light-grey text, unreadable). Light pages keep white fields.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(head: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html>{head}<style>body {{ margin: 0 }} input, textarea {{ position: absolute; left: 10px; width: 100px; height: 30px; border: 0; padding: 0 }}
             input {{ top: 10px }} textarea {{ top: 60px }}</style><input><textarea></textarea>"
        ),
        base_url: "http://test/".into(),
        width: 200,
        height: 120,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

#[test]
fn dark_meta_gives_fields_the_dark_field_colour() {
    let dark = render("<meta name=color-scheme content=dark>");
    for (what, y) in [("input", 25), ("textarea", 75)] {
        let [r, g, b] = pixel(&dark, 60, y);
        assert!(r < 100 && g < 100 && b < 100, "{what} dark: {:?}", [r, g, b]);
    }
    let light = render("");
    for (what, y) in [("input", 25), ("textarea", 75)] {
        assert_eq!(pixel(&light, 60, y), [255, 255, 255], "{what} light");
    }
}
