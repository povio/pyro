use super::ElementCx;
use crate::color::{Color, ToColorColor as _};
use anyrender::PaintScene;
use blitz_dom::CopyStyleGetters;
use blitz_dom::local_name;
use kurbo::{Affine, BezPath, Cap, Circle, Join, Point, RoundedRect, Stroke, Vec2};
use peniko::Fill;
use style::dom::TElement as _;

impl ElementCx<'_, '_> {
    pub(super) fn draw_input(&self, scene: &mut impl PaintScene) {
        if self.element.name.ns != blitz_dom::ns!(html) {
            return;
        }
        if matches!(&**self.node.local_name(), "progress" | "meter") {
            self.draw_bar_widget(scene);
            return;
        }
        if self.node.local_name() != "input" {
            return;
        }
        if self.node.attr(local_name!("type")).is_some_and(|t| t.eq_ignore_ascii_case("range")) {
            self.draw_bar_widget(scene);
            return;
        }
        let Some(checked) = self.element.checkbox_input_checked() else {
            return;
        };

        let type_attr = self.node.attr(local_name!("type"));
        let disabled = self.node.attr(local_name!("disabled")).is_some();

        // pyro: `accent-color` when it is a colour (Stylo parses it for servo since lane F1), else
        // the text colour as before.
        let accent_color = if disabled {
            Color::from_rgba8(209, 209, 209, 255)
        } else {
            self.accent_color().unwrap_or_else(|| self.style.clone_color().as_srgb_color())
        };

        let width = self.frame.border_box.width();
        let height = self.frame.border_box.height();
        let min_dimension = width.min(height);
        let scale = (min_dimension - 4.0).max(0.0) / 16.0;

        let frame = self.frame.border_box.to_rounded_rect(scale * 2.0);

        match type_attr {
            Some("checkbox") => {
                draw_checkbox(scene, checked, frame, self.transform, accent_color, scale);
            }
            Some("radio") => {
                let center = frame.center();
                draw_radio_button(scene, checked, center, self.transform, accent_color, scale);
            }
            _ => {}
        }
    }
}

fn draw_checkbox(
    scene: &mut impl PaintScene,
    checked: bool,
    frame: RoundedRect,
    transform: Affine,
    accent_color: Color,
    scale: f64,
) {
    if checked {
        scene.fill(Fill::NonZero, transform, accent_color, None, &frame);
        //Tick code derived from masonry
        let mut path = BezPath::new();
        path.move_to((2.0, 9.0));
        path.line_to((6.0, 13.0));
        path.line_to((14.0, 2.0));

        path.apply_affine(Affine::translate(Vec2 { x: 2.0, y: 1.0 }).then_scale(scale));

        let style = Stroke {
            width: 2.0 * scale,
            join: Join::Round,
            miter_limit: 10.0,
            start_cap: Cap::Round,
            end_cap: Cap::Round,
            dash_pattern: Default::default(),
            dash_offset: 0.0,
        };

        scene.stroke(&style, transform, Color::WHITE, None, &path);
    } else {
        scene.fill(Fill::NonZero, transform, Color::WHITE, None, &frame);
        scene.stroke(&Stroke::default(), transform, accent_color, None, &frame);
    }
}

fn draw_radio_button(
    scene: &mut impl PaintScene,
    checked: bool,
    center: Point,
    transform: Affine,
    accent_color: Color,
    scale: f64,
) {
    let outer_ring = Circle::new(center, 8.0 * scale);
    let gap = Circle::new(center, 6.0 * scale);
    let inner_circle = Circle::new(center, 4.0 * scale);
    if checked {
        scene.fill(Fill::NonZero, transform, accent_color, None, &outer_ring);
        scene.fill(Fill::NonZero, transform, Color::WHITE, None, &gap);
        scene.fill(Fill::NonZero, transform, accent_color, None, &inner_circle);
    } else {
        const GRAY: Color = color::palette::css::GRAY;
        scene.fill(Fill::NonZero, transform, GRAY, None, &outer_ring);
        scene.fill(Fill::NonZero, transform, Color::WHITE, None, &gap);
    }
}

impl ElementCx<'_, '_> {
    /// pyro: the used `accent-color` when it is a colour, `None` for `auto`. A (partly)
    /// transparent one is precomposed over the canvas colour of the used colour scheme (CSS UI 4
    /// § 6.1), the `Canvas` system colour (stylo device/servo.rs).
    fn accent_color(&self) -> Option<Color> {
        match &self.style.get_inherited_ui().accent_color {
            style::values::generics::color::GenericColorOrAuto::Color(c) => {
                let [r, g, b, a] = c.resolve_to_absolute(&self.style.clone_color()).as_srgb_color().components;
                let canvas = if self.dark_color_scheme() { 30.0 / 255.0 } else { 1.0 };
                let over = |v: f32| v * a + canvas * (1.0 - a);
                Some(Color::new([over(r), over(g), over(b), 1.0]))
            }
            style::values::generics::color::GenericColorOrAuto::Auto => None,
        }
    }

    fn number_attr(&self, name: blitz_dom::LocalName) -> Option<f64> {
        self.node.attr(name).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
    }

    /// pyro: `<input type=range>`, `<progress>` and `<meter>` as Chromium's form controls draw
    /// them (`NativeThemeBase::PaintSliderTrack`/`PaintSliderThumb`/`PaintProgressBar`, measured
    /// on headless Chrome): an 8px pill track across the content box, filled from the start up
    /// to the value with the accent colour (a meter's region colour), a 1px border over it, and
    /// for a range a 16px thumb of the accent colour at the value. Horizontal only.
    fn draw_bar_widget(&self, scene: &mut impl PaintScene) {
        let dark = self.dark_color_scheme();
        let name: &str = self.node.local_name();
        let disabled = name == "input" && self.node.attr(local_name!("disabled")).is_some();
        let default_accent = if dark { Color::from_rgba8(0x99, 0xc8, 0xff, 255) } else { Color::from_rgba8(0, 0x75, 0xff, 255) };
        let (track, border) = match (dark, disabled) {
            (_, true) => (Color::from_rgba8(0xfa, 0xfa, 0xfa, 255), Color::from_rgba8(0x76, 0x76, 0x76, 77)),
            (true, false) => (Color::from_rgba8(0x3b, 0x3b, 0x3b, 255), Color::from_rgba8(0x85, 0x85, 0x85, 255)),
            (false, false) => (Color::from_rgba8(0xef, 0xef, 0xef, 255), Color::from_rgba8(0x76, 0x76, 0x76, 128)),
        };
        let content = self.frame.content_box;
        let (x0, x1) = (content.x0, content.x1);
        let mid = (content.y0 + content.y1) / 2.0;
        // The fill's share of the track and its colour (`None`: an indeterminate progress bar).
        let (share, fill, border, track_inset) = match name {
            "progress" => {
                let max = self.number_attr(local_name!("max")).filter(|m| *m > 0.0).unwrap_or(1.0);
                let share = self.number_attr(local_name!("value")).map(|v| (v.max(0.0) / max).min(1.0));
                (share, self.accent_color().unwrap_or(default_accent), border, 0.0)
            }
            "meter" => {
                let min = self.number_attr(local_name!("min")).unwrap_or(0.0);
                let max = self.number_attr(local_name!("max")).unwrap_or(1.0).max(min);
                let value = self.number_attr(local_name!("value")).unwrap_or(0.0).clamp(min, max);
                let low = self.number_attr(local_name!("low")).unwrap_or(min).clamp(min, max);
                let high = self.number_attr(local_name!("high")).unwrap_or(max).clamp(low, max);
                let optimum = self.number_attr(local_name!("optimum")).unwrap_or((min + max) / 2.0).clamp(min, max);
                // HTML § 4.10.14: the optimum, suboptimal and even less good regions.
                let region = if optimum < low {
                    if value <= low { 0 } else if value <= high { 1 } else { 2 }
                } else if optimum > high {
                    if value >= high { 0 } else if value >= low { 1 } else { 2 }
                } else if value >= low && value <= high {
                    0
                } else {
                    1
                };
                let fill = match region {
                    0 => Color::from_rgba8(0x10, 0x7c, 0x10, 255),
                    1 => Color::from_rgba8(0xff, 0xb9, 0x00, 255),
                    _ => Color::from_rgba8(0xd8, 0x3b, 0x01, 255),
                };
                let share = if max > min { (value - min) / (max - min) } else { 0.0 };
                (Some(share), fill, Color::from_rgba8(0xcb, 0xcb, 0xcb, 255), 0.0)
            }
            _ => {
                let min = self.number_attr(local_name!("min")).unwrap_or(0.0);
                let max = self.number_attr(local_name!("max")).unwrap_or(100.0).max(min);
                let step = self.number_attr(local_name!("step")).filter(|s| *s > 0.0).unwrap_or(1.0);
                let value = self.number_attr(local_name!("value")).unwrap_or(min + (max - min) / 2.0).clamp(min, max);
                let value = (min + ((value - min) / step).round() * step).clamp(min, max);
                let share = if max > min { (value - min) / (max - min) } else { 0.0 };
                let accent = if disabled {
                    Color::from_rgba8(0xcb, 0xcb, 0xcb, 255)
                } else {
                    self.accent_color().unwrap_or(default_accent)
                };
                (Some(share), accent, border, 1.0)
            }
        };
        if x1 - x0 <= 2.0 * track_inset || content.height() <= 0.0 {
            return;
        }
        let half = 4.0f64.min(content.height() / 2.0);
        let track_rect = kurbo::Rect::new(x0 + track_inset, mid - half, x1 - track_inset, mid + half);
        let pill = track_rect.to_rounded_rect(half);
        scene.fill(Fill::NonZero, self.transform, track, None, &pill);
        let thumb_x = share.map(|share| {
            if name == "input" {
                x0 + 8.0 + share * (x1 - x0 - 16.0).max(0.0)
            } else {
                x0 + share * (x1 - x0)
            }
        });
        if let Some(fill_end) = thumb_x {
            // The value bar within the pill: round at the start, and at the end where it reaches
            // into the end cap.
            let end = fill_end.clamp(track_rect.x0, track_rect.x1);
            if end > track_rect.x0 {
                let start_radius = half.min(end - track_rect.x0);
                let end_radius = (half - (track_rect.x1 - end)).max(0.0);
                let radii = kurbo::RoundedRectRadii::new(start_radius, end_radius, end_radius, start_radius);
                let value = kurbo::Rect::new(track_rect.x0, track_rect.y0, end, track_rect.y1).to_rounded_rect(radii);
                scene.fill(Fill::NonZero, self.transform, fill, None, &value);
            }
        }
        let ring = track_rect.inset(-0.5).to_rounded_rect(half - 0.5);
        scene.stroke(&Stroke::new(1.0), self.transform, border, None, &ring);
        if name == "input"
            && let Some(x) = thumb_x
        {
            let radius = 8.0f64.min(content.height() / 2.0);
            scene.fill(Fill::NonZero, self.transform, fill, None, &Circle::new(Point::new(x, mid), radius));
        }
    }
}
