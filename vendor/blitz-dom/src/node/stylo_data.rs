use std::fmt;
use std::ops::Deref;
use std::{cell::UnsafeCell, ops::DerefMut};
use style::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};
use style::servo_arc::Arc;

use crate::layout::damage::ALL_DAMAGE;

/// Interior-mutable wrapper around `Option<ElementDataWrapper>`.
///
/// Encapsulates the `UnsafeCell` so that access sites don't need raw `unsafe` blocks.
///
/// Safety relies on:
///   - Regular static-borrow checking for regular access, and on:
///   - Stylo having exclusive access to nodes during style traversals
///   - Stylo's thread-safe traversal model: `init`/`clear` only happen during exclusive-access phases
pub struct StyloData {
    inner: UnsafeCell<Option<ElementDataWrapper>>,
}

impl Default for StyloData {
    fn default() -> Self {
        Self {
            inner: UnsafeCell::new(None),
        }
    }
}

impl fmt::Debug for StyloData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StyloData").finish_non_exhaustive()
    }
}

impl Deref for StyloData {
    type Target = Option<ElementDataWrapper>;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.inner.get() }
    }
}

impl DerefMut for StyloData {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.get_mut()
    }
}

impl StyloData {
    /// Whether element data has been initialized.
    pub fn has_data(&self) -> bool {
        unsafe { &*self.inner.get() }.is_some()
    }

    /// Borrow the element data immutably, if present.
    pub fn get(&self) -> Option<ElementDataRef<'_>> {
        self.as_ref().map(|w| w.borrow())
    }

    /// Borrow the element data mutably, if present.
    pub fn get_mut(&mut self) -> Option<ElementDataMut<'_>> {
        self.as_mut().map(|w| w.borrow_mut())
    }

    /// Initialize the element data ready for use (if it is not already initialized)
    pub fn ensure_init_mut(&mut self) -> ElementDataMut<'_> {
        // SAFETY:
        // If we have exclusive access to self (implied by &mut self) then it safe to mutate self.
        unsafe { self.ensure_init() }
    }

    pub fn primary_styles(&self) -> Option<StyleDataRef<'_>> {
        let stylo_element_data = self.get();
        if stylo_element_data
            .as_ref()
            .and_then(|d| d.styles.get_primary())
            .is_some()
        {
            Some(StyleDataRef(self.get().unwrap()))
        } else {
            None
        }
    }

    /// Borrow the primary computed style directly (rather than the `Arc` containing it).
    pub fn computed_styles(&self) -> Option<ComputedStyleRef<'_>> {
        self.primary_styles().map(|primary| ComputedStyleRef { primary })
    }

    /// Get a mutable reference to the data
    pub unsafe fn unsafe_stylo_only_mut(&self) -> Option<ElementDataMut<'_>> {
        let opt = unsafe { &mut *self.inner.get() };
        opt.as_mut().map(|w| w.borrow_mut())
    }

    /// Initialize the element data ready for use (if it is not already initialized)
    ///
    /// SAFETY:
    /// There must be no outstanding borrows to this container or anything contained within it
    /// when this method is called
    pub unsafe fn ensure_init(&self) -> ElementDataMut<'_> {
        if !self.has_data() {
            unsafe { *self.inner.get() = Some(ElementDataWrapper::default()) };
            let mut data_mut = unsafe { self.unsafe_stylo_only_mut() }.unwrap();
            data_mut.damage = ALL_DAMAGE;
            data_mut
        } else {
            unsafe { self.unsafe_stylo_only_mut() }.unwrap()
        }
    }

    /// Clear the element data, returning to the uninitialized state.
    ///
    /// SAFETY:
    /// There must be no outstanding borrows to this container or anything contained within it
    /// when this method is called
    pub unsafe fn clear(&self) {
        unsafe { *self.inner.get() = None };
    }
}

pub struct StyleDataRef<'a>(ElementDataRef<'a>);

impl Deref for StyleDataRef<'_> {
    type Target = Arc<style::properties::ComputedValues>;

    fn deref(&self) -> &Self::Target {
        self.0.styles.get_primary().unwrap()
    }
}

/// Borrow of a node's stylo element data that `Deref`s directly to the primary
/// [`ComputedValues`](style::properties::ComputedValues), suitable for wrapping
/// in [`stylo_taffy::TaffyStyloStyle`].
pub struct ComputedStyleRef<'a> {
    primary: StyleDataRef<'a>,
}

impl ComputedStyleRef<'_> {
    /// The layout-derived replacements of a table cell's style: the primary style is never
    /// copied or changed, `TaffyStyloStyle`'s `size` and `border` getters read these instead.
    pub(crate) fn table_cell_override(&self, collapsed_border: Option<[f32; 4]>) -> stylo_taffy::CellOverride {
        use style::computed_values::border_collapse::T as BorderCollapse;
        use style::values::specified::box_::DisplayInside;
        if self.get_box().display.inside() != DisplayInside::TableCell {
            return stylo_taffy::CellOverride::default();
        }
        // TableCell retains this length as an intrinsic border-box minimum.
        // A definite height here would prevent the cell from growing to fit.
        let auto_height = stylo_taffy::convert::dimension(&self.get_position().height).into_option().is_some();
        // Collapsed borders live in the table's grid gutters. They must not
        // also consume space inside the cell's own formatting context.
        // In a table without uniform borders a cell keeps half of the ones around it.
        let border = (self.get_inherited_table().border_collapse == BorderCollapse::Collapse).then(|| {
            // Rounded to app units, as a width stored in the style would be.
            collapsed_border.unwrap_or_default().map(|px| app_units::Au::from_f32_px(px).to_f32_px())
        });
        stylo_taffy::CellOverride { auto_height, border }
    }

    /// The style `table_cell_override` stands for: the primary style copied with the cell's
    /// height and borders replaced. Only for checking the override against.
    #[cfg(test)]
    pub(crate) fn derived_table_cell_style(
        &self,
        collapsed_border: Option<[f32; 4]>,
    ) -> Option<style::properties::ComputedValues> {
        use style::computed_values::border_collapse::T as BorderCollapse;
        use style::values::specified::box_::DisplayInside;
        if self.clone_display().inside() != DisplayInside::TableCell {
            return None;
        }
        let numeric_height = stylo_taffy::convert::dimension(&self.clone_height()).into_option().is_some();
        let collapsed = self.clone_border_collapse() == BorderCollapse::Collapse;
        if !numeric_height && !collapsed {
            return None;
        }
        let mut derived = (**self).clone();
        if numeric_height {
            derived.mutate_position().height = style::values::computed::Size::auto();
        }
        if collapsed {
            let border = derived.mutate_border();
            let [top, right, bottom, left] = collapsed_border.unwrap_or_default();
            let au = app_units::Au::from_f32_px;
            border.border_top_width.0 = au(top);
            border.border_right_width.0 = au(right);
            border.border_bottom_width.0 = au(bottom);
            border.border_left_width.0 = au(left);
            if collapsed_border.is_some() {
                use style::values::computed::BorderStyle;
                border.border_top_style = BorderStyle::Solid;
                border.border_right_style = BorderStyle::Solid;
                border.border_bottom_style = BorderStyle::Solid;
                border.border_left_style = BorderStyle::Solid;
            }
        }
        Some(derived)
    }
}

impl Deref for ComputedStyleRef<'_> {
    type Target = style::properties::ComputedValues;

    fn deref(&self) -> &Self::Target {
        self.primary.0.styles.get_primary().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use crate::{Attribute, BaseDocument, DocumentConfig, qual_name};
    use blitz_traits::shell::{ColorScheme, Viewport};
    use taffy::CoreStyle;

    /// Every styled node's layout style reads the same `size` and `border` as the copied cell
    /// style it replaced: separate and collapsed tables, uniform and per-cell collapsed borders,
    /// fixed, percentage and auto cell heights, and a vertical cell.
    #[test]
    fn a_table_cell_override_reads_as_the_copied_style_did() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let style = |value: &str| Attribute { name: qual_name!("style"), value: value.to_string() };
        let tables = [
            ("border-collapse:collapse", ["height:30px;border:2px solid", "border:2px solid", "height:10%;border:2px solid"]),
            ("border-collapse:collapse", ["height:30px;border:3px solid", "border:1px dotted", "border:5px double;writing-mode:vertical-rl"]),
            ("border-collapse:collapse;border:4px solid", ["border:none", "height:auto", "height:12.5px;border:1.5px solid"]),
            ("border-collapse:separate", ["height:30px;border:2px solid", "border:1px solid", "height:min-content"]),
        ];
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        for (table_style, cells) in tables {
            let table = mutator.create_element(qual_name!("table"), vec![style(&format!("display:table;{table_style}"))]);
            let tbody = mutator.create_element(qual_name!("tbody"), vec![style("display:table-row-group")]);
            for _ in 0..2 {
                let tr = mutator.create_element(qual_name!("tr"), vec![style("display:table-row")]);
                for cell in cells {
                    let td = mutator.create_element(qual_name!("td"), vec![style(&format!("display:table-cell;{cell}"))]);
                    let text = mutator.create_text_node("cell");
                    mutator.append_children(td, &[text]);
                    mutator.append_children(tr, &[td]);
                }
                mutator.append_children(tbody, &[tr]);
            }
            mutator.append_children(table, &[tbody]);
            mutator.append_children(body, &[table]);
        }
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);
        doc.resolve(0.0);

        let (mut overridden, mut per_cell, mut uniform) = (0, 0, 0);
        for (_, node) in doc.nodes.iter() {
            let Some(primary) = node.try_stylo_element_data().and_then(|data| data.computed_styles()) else {
                continue;
            };
            let collapsed_border = node.collapsed_border();
            let layout = node.layout_style();
            let Some(derived) = primary.derived_table_cell_style(collapsed_border) else {
                assert_eq!(layout.cell, stylo_taffy::CellOverride::default(), "{}", node.node_debug_str());
                continue;
            };
            overridden += 1;
            if layout.cell.border.is_some() {
                if collapsed_border.is_some() { per_cell += 1 } else { uniform += 1 }
            }
            let mut reference = stylo_taffy::TaffyStyloStyle::new(&derived, layout.flags);
            #[cfg(feature = "writing-mode")]
            {
                reference.layout_wm = layout.layout_wm;
            }
            let what = node.node_debug_str();
            assert_eq!(layout.size(), reference.size(), "{what}");
            assert_eq!(layout.border(), reference.border(), "{what}");
            assert_eq!(layout.min_size(), reference.min_size(), "{what}");
            assert_eq!(layout.max_size(), reference.max_size(), "{what}");
            assert_eq!(layout.padding(), reference.padding(), "{what}");
            let (layout, reference): (taffy::Style<style::Atom>, taffy::Style<style::Atom>) =
                (layout.into(), reference.into());
            assert_eq!(layout.size, reference.size, "{what}");
            assert_eq!(layout.border, reference.border, "{what}");
        }
        eprintln!("{overridden} overridden, {per_cell} per-cell borders, {uniform} uniform");
        // The first table's borders are uniform, the next two's per cell; the last only has heights.
        assert!(overridden >= 20, "{overridden}");
        assert!(per_cell >= 12, "{per_cell}");
        assert!(uniform >= 6, "{uniform}");
    }
}
