//! A negative `outline-offset` pulls the outline in no further than to the middle of the box, in
//! each axis apart (WPT css-ui `outline-014`, `-015`): a 200x100 box's outline 50px wide, offset
//! -100px, is a 100x100 square in its middle.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn green_columns() -> (u32, u32, u32, u32) {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: "<!doctype html><style>body{margin:0} div{margin:20px;width:200px;height:100px;outline:solid 50px #00a000;outline-offset:-100px}</style><div></div>".into(),
        base_url: "http://test/".into(),
        width: 400,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..img.height {
        for x in 0..img.width {
            let p = &img.data[((y * img.width + x) * 4) as usize..][..3];
            if p == [0, 160, 0] {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    (x0, y0, x1, y1)
}

#[test]
fn a_negative_offset_meets_in_the_middle_of_each_axis() {
    // The box is 20..220 x 20..120: the square is 70..170 x 20..120.
    assert_eq!(green_columns(), (70, 20, 170, 120));
}
