//! Integration tests, linked into one binary: each test executable links the
//! whole renderer (and is re-signed on macOS), so one per file made `cargo test`
//! mostly link time. Add new test files as modules here.

mod borders_backgrounds;
mod br_flex_item;
mod container_queries;
mod containing_block_chain;
mod dotted_dashed_lines;
mod filter_drop_shadow;
mod font_optical_sizing;
mod font_unicode_range;
mod fractional_scroll_overflow;
mod hyphen_digit_breaks;
mod image_input_replaced;
mod incremental_construction;
mod incremental_flex_overflow;
mod inline_box_decorations;
mod inline_svg_viewport;
mod input_percentage_min_content;
mod input_placeholder;
mod lang_pseudo_class;
mod line_break_resume;
mod line_height_normal;
mod mirror;
mod opacity_group;
mod out_of_flow_boundary;
mod pdf_fidelity;
mod pdf_variable_fonts;
mod profiling_timers;
mod render;
mod sampled_animations;
mod style_sheet_charset;
mod svg_reference_cycles;
mod table_cell_height;
mod table_cell_width;
mod table_collapsed_borders;
mod table_flex_intrinsic_height;
mod tagged_images;
mod text_aliasing;
mod text_line_tops;
mod text_shadow;
mod text_overflow_ellipsis;
mod vertical_writing_mode;
