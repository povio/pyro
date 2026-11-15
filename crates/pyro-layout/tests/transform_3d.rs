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

/// pyro (lane H2): a box turned out of the plane under a perspective reports the bounds of the
/// trapezoid paint draws through its 4x4 matrix, and is hit there: its near edge reaches above
/// the affine that stands in for it (whose bounds were reported). Every hit-test path agrees.
#[test]
fn layout_queries_follow_a_plane_drawn_through_a_perspective() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0 }
          div { position: absolute }
          #persp { left: 100px; top: 50px; width: 150px; height: 150px; perspective: 500px }
          #rot { left: 0; top: 0; width: 150px; height: 150px; background: #cfc; transform: rotateY(50deg) }
        </style></head><body>
        <div id=persp><div id=rot></div></div>
        </body></html>",
    );
    let rot = p.id("rot");
    // About the centre (175, 125): a point u px right of it moves to depth -u sin 50deg, where
    // W = 1 + u sin 50deg / 500.
    let (sin, cos) = (50f64.to_radians().sin(), 50f64.to_radians().cos());
    let w = |u: f64| 1.0 + u * sin / 500.0;
    let (left, right) = (175.0 - 75.0 * cos / w(-75.0), 175.0 + 75.0 * cos / w(75.0));
    let (top, bottom) = (125.0 - 75.0 / w(-75.0), 125.0 + 75.0 / w(-75.0));
    let r = p.layout.bounding_client_rect(&p.dom, rot);
    for (got, want) in [(r.x, left), (r.y, top), (r.width, right - left), (r.height, bottom - top)] {
        assert!((got - want).abs() < 1.0 / 32.0, "{r:?}: {got} for {want}");
    }
    let at = |x, y| p.describe(p.layout.element_from_point(&p.dom, x, y));
    // Above the stand-in (which starts at y = 49) but inside the near edge.
    assert_eq!(at(122.0, 42.0), p.describe(Some(rot)));
    assert!(p.compare("a plane drawn through a perspective") >= 2);
}

/// pyro (lane H2): in a `preserve-3d` element turned under a perspective (drawn through its
/// homography), a child plane (`translateZ`) is reported where paint draws it, its own projection
/// (the parent's perspective applied once), and an untransformed child at its place in the
/// parent's plane. They were composed through the parent's homography after the inverse of the
/// affine that stands in for it: the face 13px too narrow, the title 7px off. Expected values are
/// the corners through each box's 4x4 matrix, computed independently.
#[test]
fn layout_queries_follow_the_planes_of_a_context_under_a_perspective() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0 }
          div { position: absolute; left: 0; top: 0; width: 200px; height: 200px }
          #scene { left: 100px; top: 50px; perspective: 600px }
          #cube { transform-style: preserve-3d; transform: rotateY(30deg); background: #eee }
          #title { left: 20px; top: 20px; width: 60px; height: 40px; background: #ccf }
          #face { transform: translateZ(100px); background: #cfc8 }
        </style></head><body>
        <div id=scene><div id=cube><div id=title></div><div id=face></div></div></div>
        </body></html>",
    );
    let rect = |id| {
        let r = p.layout.bounding_client_rect(&p.dom, p.id(id));
        [r.x, r.y, r.x + r.width, r.y + r.height]
    };
    for (id, want) in [
        ("cube", [105.52, 40.91, 279.94, 259.09]),
        ("face", [152.61, 20.52, 345.48, 279.48]),
        ("title", [125.77, 64.29, 182.39, 109.32]),
    ] {
        let got = rect(id);
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.04), "#{id}: {got:?} for {want:?}");
    }
    let at = |x, y| p.describe(p.layout.element_from_point(&p.dom, x, y));
    // Inside the face's right edge (345.5), past where it was reported (332.4).
    assert_eq!(at(340.0, 150.0), p.describe(Some(p.id("face"))));
    assert!(p.compare("the planes of a context under a perspective") >= 3);
}

/// pyro (lane H2): a box crossing the viewer's plane is drawn in part (the part in front of the
/// viewer), but its projection has no bounds: layout keeps its untransformed box. A stand-in
/// spanning the projection made the document 1e5 px tall and the box 1e7 px.
#[test]
fn a_box_crossing_the_viewer_keeps_its_box_for_layout() {
    let p = page(
        "<!doctype html><html><head><style>
          body { margin: 0 }
          div { position: absolute; left: 0; top: 0 }
          #persp { width: 400px; height: 300px; perspective: 200px; perspective-origin: 200px 50px }
          #floor { top: 150px; width: 400px; height: 400px; background: green; transform-origin: 50% 0; transform: rotateX(90deg) }
        </style></head><body>
        <div id=persp><div id=floor></div></div>
        </body></html>",
    );
    let r = p.layout.bounding_client_rect(&p.dom, p.id("floor"));
    assert_eq!([r.x, r.y, r.width, r.height], [0.0, 150.0, 400.0, 400.0]);
    let html = p.dom.document_element(p.doc).unwrap().unwrap();
    assert_eq!(p.layout.scroll_height(&p.dom, html), 600.0);
    assert_eq!(p.layout.scroll_width(&p.dom, html), 800.0);
}
