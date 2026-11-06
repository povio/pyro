//! The diagnostic timers stay off until a client asks for them. The only test
//! here that enables them: `profiling::enable` is process-wide and cannot be
//! turned off, and the phase timers are per thread (each test has its own).

use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;

#[test]
fn phase_timers_run_only_once_profiling_is_enabled() {
    let mut doc = HtmlDocument::from_html(
        "<!doctype html><style>#t{width:80px;height:20px}</style><div id=t>text</div>",
        DocumentConfig::default(),
    );
    let passes = lp_render::blitz_passes();
    doc.resolve(0.0);
    assert!(!lp_render::profiling::on());
    assert!(lp_render::blitz_passes().1 > passes.1, "the counters count while the timers are off");
    assert_eq!(lp_render::blitz_phases(), [0.0; 9], "no phase is timed while profiling is off");

    lp_render::profiling::enable();
    assert!(lp_render::profiling::on());
    let t = doc.query_selector("#t").unwrap().unwrap();
    doc.set_style_property(t, "width", "120px");
    doc.resolve(0.0);
    let phases = lp_render::blitz_phases();
    assert!(phases[0] > 0.0 && phases[4] > 0.0, "style and layout timed once enabled: {phases:?}");
}
