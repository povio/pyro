//! `getBoundingClientRect` and `elementFromPoint` follow a 3D transform as paint flattens it:
//! a box rotated out of the plane is narrower, a box moved towards the viewer under its
//! containing block's perspective is larger (they kept the untransformed box). Every hit-test
//! path (the scan, the descent, the index) answers the same.
use crate::hit_descent::page;

#[test]
fn layout_queries_follow_flattened_3d_transforms() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0 }
          div { position: absolute }
          #rot { left: 100px; top: 20px; width: 100px; height: 40px; background: #cfc; transform: rotateY(60deg) }
          #persp { left: 100px; top: 100px; width: 150px; height: 150px; perspective: 3px }
          #near { left: 25px; top: 25px; width: 100px; height: 100px; background: #ccf; transform: translateZ(1px) }
        </style></head><body>
        <div id=rot></div>
        <div id=persp><div id=near></div></div>
        </body></html>",
    );
    let (rot, near) = (p.id("rot"), p.id("near"));
    let rect = |id| {
        let r = p.layout.bounding_client_rect(&p.dom, id);
        [r.x, r.y, r.width, r.height].map(|v| (v * 100.0).round() / 100.0)
    };
    assert_eq!(rect(rot), [125.0, 20.0, 50.0, 40.0]);
    assert_eq!(rect(near), [100.0, 100.0, 150.0, 150.0]);

    let at = |x, y| p.describe(p.layout.element_from_point(&p.dom, x, y));
    // Inside the untransformed box but outside its projection: not the box.
    assert_ne!(at(110.0, 40.0), p.describe(Some(rot)));
    assert_eq!(at(150.0, 40.0), p.describe(Some(rot)));
    // Outside the untransformed box (25..125) but inside the scaled one (0..150).
    assert_eq!(at(110.0, 110.0), p.describe(Some(near)));
    assert_eq!(at(240.0, 240.0), p.describe(Some(near)));
    assert!(p.compare("flattened 3d transforms") >= 3);
}

/// An absolutely positioned cell of a collapsed table is reported with its share of the
/// collapsed borders (gutters) beyond its layout box. Its row does not hold it, so the descent
/// reaches it through its containing block, by a bound that left the gutters out: a point in them
/// hit what lay below (a random page of `hit_index` met it once its table's `translate3d` took
/// effect).
#[test]
fn an_out_of_flow_cell_is_hit_in_its_collapsed_border_gutters() {
    for (label, table) in [("list-item table", "display:list-item;"), ("table", "")] {
        let p = page(&format!(
            "<!doctype html><html><head><style>
              body {{ margin: 0; font: 13px/15px sans-serif }}
              #under {{ position: absolute; left: 0; top: 0; width: 500px; height: 400px; background: #eee }}
              table {{ margin-left: 200px; height: 125px; transform: translate(40px, -30px); border-collapse: collapse; {table} }}
              #c {{ width: 200px; position: absolute; left: -137px; top: -87px; padding: 12px; border: 5px solid #888 }}
            </style></head><body>
            <div id=under></div><div style='height:300px'></div>
            <table><tr><td id=c>cell</td></tr><tr><td>b</td><td>c</td></tr></table>
            </body></html>"
        ));
        let c = p.id("c");
        let r = p.layout.bounding_client_rect(&p.dom, c);
        let at = |x, y| p.describe(p.layout.element_from_point_descent(&p.dom, x, y));
        let scan = |x, y| p.describe(p.layout.element_from_point_scan(&p.dom, x, y));
        // Just inside the reported box's edges, in the gutters.
        for (x, y) in [(r.x + 1.0, r.y + 1.0), (r.x + r.width / 2.0, r.y + 0.5), (r.x + 0.5, r.y + r.height / 2.0)] {
            assert_eq!(scan(x, y), p.describe(Some(c)), "{label}: scan at ({x}, {y}) of {r:?}");
            assert_eq!(at(x, y), scan(x, y), "{label}: descent at ({x}, {y}) of {r:?}");
        }
    }
}
