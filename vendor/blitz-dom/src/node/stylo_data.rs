use std::fmt;
use stylo_taffy::CopyStyleGetters;
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
    /// pyro: the primary style in the forms layout reads it in, rewritten whenever the
    /// primary style is replaced (see [`LayoutStyles`]).
    layout: UnsafeCell<LayoutStyles>,
}

impl Default for StyloData {
    fn default() -> Self {
        Self {
            inner: UnsafeCell::new(None),
            layout: UnsafeCell::new(LayoutStyles::default()),
        }
    }
}

/// pyro: what layout reads of a node's primary style, derived from it when it is
/// written instead of on every layout pass. The style traversal (`RecalcStyle::process_preorder`,
/// `sync_pseudo_element_styles`) rewrites it when a primary style is replaced with restyle damage
/// that reaches layout ([`LayoutStyles::affected_by`]); `Node::update_layout_styles` wherever a
/// primary style is set outside the traversal; and `Node::set_collapsed_border` a cell's
/// [`LayoutStyles::cell`] when table construction changes its collapsed borders.
#[derive(Default)]
pub(crate) struct LayoutStyles {
    /// The physical Taffy style of a table, table row, column or cell
    /// ([`stylo_taffy::to_taffy_style`]), which table construction places cells and sizes tracks
    /// from.
    pub(crate) table_part: Option<Box<taffy::Style<style::Atom>>>,
    /// A table cell's style as its own layout reads it, where that differs from the primary
    /// style (see [`cell_layout_style`]).
    pub(crate) cell: Option<Box<style::properties::ComputedValues>>,
}

impl LayoutStyles {
    /// The layout styles of a node whose primary style is `style` and whose half collapsed
    /// borders (a cell of a table without uniform ones) are `collapsed_border`.
    pub(crate) fn derive(
        style: &style::properties::ComputedValues,
        collapsed_border: Option<[f32; 4]>,
    ) -> Self {
        Self {
            table_part: table_part_style(style),
            cell: cell_layout_style(style, collapsed_border),
        }
    }

    /// Whether a restyle with `damage` can change the layout styles derived from the primary
    /// style. Every property they read (the ones `stylo_taffy` converts, `border-collapse`,
    /// `height`, the border widths and styles) gives Stylo's `RELAYOUT` damage when it changes;
    /// paint-only changes (colour, background, outline, opacity, transforms) do not, and leave
    /// the stored layout styles equal to what the new style gives.
    #[inline]
    pub(crate) fn affected_by(damage: style::selector_parser::RestyleDamage) -> bool {
        damage.intersects(crate::layout::damage::ONLY_RELAYOUT)
    }

    /// Debug builds: these are the layout styles `style` and `collapsed_border` give.
    #[cfg(debug_assertions)]
    pub(crate) fn assert_current(
        &self,
        style: &style::properties::ComputedValues,
        collapsed_border: Option<[f32; 4]>,
        node_id: blitz_traits::node_id::NodeId,
    ) {
        let fresh = Self::derive(style, collapsed_border);
        assert_eq!(
            self.table_part, fresh.table_part,
            "stale table-part Taffy style on node {node_id:?}"
        );
        let as_taffy = |cell: &Option<Box<style::properties::ComputedValues>>| {
            cell.as_deref().map(stylo_taffy::to_taffy_style)
        };
        assert_eq!(
            as_taffy(&self.cell),
            as_taffy(&fresh.cell),
            "stale table-cell layout style on node {node_id:?}"
        );
    }
}

/// The physical Taffy style of a table part (table, row, column or cell) whose primary style is
/// `style`, or `None` for any other box. Stored as [`LayoutStyles::table_part`].
fn table_part_style(style: &style::properties::ComputedValues) -> Option<Box<taffy::Style<style::Atom>>> {
    use style::values::specified::box_::DisplayInside;
    matches!(
        style.clone_display().inside(),
        DisplayInside::Table
            | DisplayInside::TableRow
            | DisplayInside::TableColumn
            | DisplayInside::TableCell
    )
    .then(|| Box::new(stylo_taffy::to_taffy_style(style)))
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
        self.primary_styles().map(|primary| ComputedStyleRef { primary, layout: None })
    }

    /// The primary computed style as the node's own layout reads it: a table cell's
    /// [`LayoutStyles::cell`] when it has one.
    pub(crate) fn layout_computed_styles(&self) -> Option<ComputedStyleRef<'_>> {
        let layout = self.layout_styles().cell.as_deref();
        self.primary_styles().map(|primary| ComputedStyleRef { primary, layout })
    }

    /// The layout styles derived from the primary style.
    #[inline]
    pub(crate) fn layout_styles(&self) -> &LayoutStyles {
        // SAFETY: written only through `&mut self`, or by the style traversal while it has
        // exclusive access to the node (see `set_layout_styles`).
        unsafe { &*self.layout.get() }
    }

    /// Mutable access to the layout styles derived from the primary style.
    pub(crate) fn layout_styles_mut(&mut self) -> &mut LayoutStyles {
        self.layout.get_mut()
    }

    /// Replace the layout styles derived from the primary style, from the style traversal.
    ///
    /// SAFETY:
    /// There must be no outstanding borrows of the layout styles. The style traversal, which has
    /// exclusive access to the node it restyles and never reads them, upholds this.
    pub(crate) unsafe fn set_layout_styles(&self, styles: LayoutStyles) {
        unsafe { *self.layout.get() = styles };
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
        unsafe { *self.layout.get() = LayoutStyles::default() };
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
    // Layout-derived table internals never replace the authored primary style.
    layout: Option<&'a style::properties::ComputedValues>,
}

/// A table cell's style as its own layout reads it, when that differs from its primary style
/// `style`. `collapsed_border` is the cell's half of the collapsed borders around it in a table
/// without uniform ones. Stored as [`LayoutStyles::cell`].
pub(crate) fn cell_layout_style(
    style: &style::properties::ComputedValues,
    collapsed_border: Option<[f32; 4]>,
) -> Option<Box<style::properties::ComputedValues>> {
    use style::computed_values::border_collapse::T as BorderCollapse;
    use style::values::specified::box_::DisplayInside;
    if style.clone_display().inside() != DisplayInside::TableCell {
        return None;
    }
    let numeric_height = stylo_taffy::convert::dimension(&style.get_position().height)
        .into_option()
        .is_some();
    let collapsed = style.clone_border_collapse() == BorderCollapse::Collapse;
    if !numeric_height && !collapsed {
        return None;
    }
    {
        let mut derived = Box::new(style.clone());
        // TableCell retains this length as an intrinsic border-box minimum.
        // A definite height here would prevent the cell from growing to fit.
        if numeric_height {
            derived.mutate_position().height = style::values::computed::Size::auto();
        }
        // Collapsed borders live in the table's grid gutters. They must not
        // also consume space inside the cell's own formatting context.
        if collapsed {
            let border = derived.mutate_border();
            // In a table without uniform borders a cell keeps half of the ones around it.
            let [top, right, bottom, left] = collapsed_border.unwrap_or_default();
            let au = app_units::Au::from_f32_px;
            border.border_top_width.0 = au(top);
            border.border_right_width.0 = au(right);
            border.border_bottom_width.0 = au(bottom);
            border.border_left_width.0 = au(left);
            // Stylo's widths mean nothing without a style.
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
        self.layout.unwrap_or_else(|| self.primary.0.styles.get_primary().unwrap())
    }
}
