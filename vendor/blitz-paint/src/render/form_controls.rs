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
        if self.node.local_name() == "select" {
            self.draw_select_arrow(scene);
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
        // pyro: `appearance: none` draws no native checkbox or radio (CSS UI 4), only the box.
        if self.native_appearance_none() {
            return;
        }

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
    /// pyro: the drop-down arrow of a native drop-down `<select>` (a menu list; a list box,
    /// `multiple` or `size` above 1, has none), a chevron in the room its intrinsic size keeps for it
    /// at the inline end (`layout/mod.rs`), centred in the block axis. `appearance: none` has none
    /// (CSS UI 4). In a vertical writing mode the inline end is the bottom (`direction: rtl`: the top).
    fn draw_select_arrow(&self, scene: &mut impl PaintScene) {
        if self.native_appearance_none() {
            return;
        }
        let listbox = self.node.attr(local_name!("multiple")).is_some()
            || self
                .node
                .attr(local_name!("size"))
                .and_then(|s| s.trim().parse::<u32>().ok())
                .is_some_and(|s| s > 1);
        if listbox {
            return;
        }
        let wm = self.style.writing_mode;
        let vertical = wm.is_vertical();
        let rtl = !wm.is_bidi_ltr();
        let padding = self.frame.padding_box;
        let s = self.scale;
        // The arrow's centre, 9.5px in from the inline-end padding edge.
        let inset = 9.5 * s;
        let center = if vertical {
            Point::new((padding.x0 + padding.x1) / 2.0, if rtl { padding.y0 + inset } else { padding.y1 - inset })
        } else {
            Point::new(if rtl { padding.x0 + inset } else { padding.x1 - inset }, (padding.y0 + padding.y1) / 2.0)
        };
        let inline_size = if vertical { padding.height() } else { padding.width() };
        if inline_size < 2.0 * inset {
            return;
        }
        let (half, depth) = (3.5 * s, 2.0 * s);
        let mut path = BezPath::new();
        path.move_to((center.x - half, center.y - depth));
        path.line_to((center.x, center.y + depth));
        path.line_to((center.x + half, center.y - depth));
        let color = self.style.clone_color().as_srgb_color();
        let stroke = Stroke::new(1.5 * s).with_caps(Cap::Round).with_join(Join::Round);
        scene.stroke(&stroke, self.transform, color, None, &path);
    }

    /// pyro: whether the element's `appearance` is `none` (no native look, CSS UI 4).
    fn native_appearance_none(&self) -> bool {
        use style::computed_values::appearance::T as Appearance;
        *self.style.get_box().get_appearance() == Appearance::None
    }

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
        // An author or user border or background on a meter or progress bar turns its native
        // look off too (CSS UI 4's appearance-disabling properties; WPT css-ui
        // compute-kind-widget-generated).
        let none = self.native_appearance_none()
            || (name != "input"
                && self
                    .style
                    .flags
                    .contains(style::computed_value_flags::ComputedValueFlags::HAS_AUTHOR_OR_USER_SPECIFIED_BORDER_BACKGROUND));
        // The control is drawn in a frame along its main axis (x, `length` long) across its
        // thickness (y), mapped onto the content box: a vertical writing mode turns it, and the
        // value runs from the inline start (right to left in rtl, bottom to top when the inline
        // direction is).
        let wm = self.style.writing_mode;
        let vertical = wm.is_vertical();
        let reversed = if vertical { !wm.is_inline_tb() } else { !wm.is_bidi_ltr() };
        let (length, thickness) = if vertical { (content.height(), content.width()) } else { (content.width(), content.height()) };
        let mut frame = Affine::translate(content.origin().to_vec2());
        if vertical {
            frame *= Affine::new([0.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
        }
        if reversed {
            frame *= Affine::new([-1.0, 0.0, 0.0, 1.0, length, 0.0]);
        }
        let transform = self.transform * frame;
        let (x0, x1) = (0.0, length);
        let mid = thickness / 2.0;
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
        if x1 - x0 <= 2.0 * track_inset || thickness <= 0.0 {
            return;
        }
        let thumb_x = |share: Option<f64>| {
            share.map(|share| if name == "input" { x0 + 8.0 + share * (x1 - x0 - 16.0).max(0.0) } else { x0 + share * (x1 - x0) })
        };
        if none {
            // Chromium's non-native controls (html.css): a progress bar is a grey rectangle filled
            // green up to the value, a meter a bar half as thick with Chromium's gradients, and a
            // range only its thumb.
            match name {
                "progress" => {
                    let bar = kurbo::Rect::new(0.0, 0.0, length, thickness);
                    scene.fill(Fill::NonZero, transform, Color::from_rgba8(0x80, 0x80, 0x80, 255), None, &bar);
                    if let Some(share) = share {
                        let value = kurbo::Rect::new(0.0, 0.0, length * share, thickness);
                        scene.fill(Fill::NonZero, transform, Color::from_rgba8(0, 0x80, 0, 255), None, &value);
                    }
                }
                "meter" => {
                    let rgb = |v: u32| Color::from_rgba8((v >> 16) as u8, (v >> 8) as u8, v as u8, 255);
                    let (y0, y1) = (thickness / 4.0, thickness * 3.0 / 4.0);
                    // Top to bottom: the edge colour, a light band at 20%, a dark band from 45%
                    // to 55%, the edge colour again.
                    let gradient = |[edge, light, dark]: [u32; 3]| {
                        let [a, b, c] = [edge, light, dark].map(rgb);
                        peniko::Gradient::new_linear((0.0, y0), (0.0, y1))
                            .with_stops([(0.0, a), (0.2, b), (0.45, c), (0.55, c), (1.0, a)].as_slice())
                    };
                    let bar = kurbo::Rect::new(0.0, y0, length, y1);
                    scene.fill(Fill::NonZero, transform, &gradient([0xdddddd, 0xeeeeee, 0xcccccc]), None, &bar);
                    let stops = if fill == Color::from_rgba8(0x10, 0x7c, 0x10, 255) {
                        [0xaadd77, 0xcceeaa, 0x77aa33]
                    } else if fill == Color::from_rgba8(0xff, 0xb9, 0x00, 255) {
                        [0xffee77, 0xffffcc, 0xddbb33]
                    } else {
                        [0xff7777, 0xffcccc, 0xdd4444]
                    };
                    let value = kurbo::Rect::new(0.0, y0, length * share.unwrap_or(0.0), y1);
                    scene.fill(Fill::NonZero, transform, &gradient(stops), None, &value);
                }
                _ => {
                    if let Some(x) = thumb_x(share) {
                        let radius = 8.0f64.min(thickness / 2.0);
                        scene.fill(Fill::NonZero, transform, fill, None, &Circle::new(Point::new(x, mid), radius));
                    }
                }
            }
            return;
        }
        let half = 4.0f64.min(thickness / 2.0);
        let track_rect = kurbo::Rect::new(x0 + track_inset, mid - half, x1 - track_inset, mid + half);
        let pill = track_rect.to_rounded_rect(half);
        scene.fill(Fill::NonZero, transform, track, None, &pill);
        let thumb_x = thumb_x(share);
        if let Some(fill_end) = thumb_x {
            // The value bar within the pill: round at the start, and at the end where it reaches
            // into the end cap.
            let end = fill_end.clamp(track_rect.x0, track_rect.x1);
            if end > track_rect.x0 {
                let start_radius = half.min(end - track_rect.x0);
                let end_radius = (half - (track_rect.x1 - end)).max(0.0);
                let radii = kurbo::RoundedRectRadii::new(start_radius, end_radius, end_radius, start_radius);
                let value = kurbo::Rect::new(track_rect.x0, track_rect.y0, end, track_rect.y1).to_rounded_rect(radii);
                scene.fill(Fill::NonZero, transform, fill, None, &value);
            }
        }
        let ring = track_rect.inset(-0.5).to_rounded_rect(half - 0.5);
        scene.stroke(&Stroke::new(1.0), transform, border, None, &ring);
        if name == "input"
            && let Some(x) = thumb_x
        {
            let radius = 8.0f64.min(thickness / 2.0);
            scene.fill(Fill::NonZero, transform, fill, None, &Circle::new(Point::new(x, mid), radius));
        }
    }
}
