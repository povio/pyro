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
