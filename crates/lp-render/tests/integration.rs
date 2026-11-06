//! Integration tests, linked into one binary: each test executable links the
//! whole renderer (and is re-signed on macOS), so one per file made `cargo test`
//! mostly link time. Add new test files as modules here.

mod br_flex_item;
mod container_queries;
mod font_optical_sizing;
mod fractional_scroll_overflow;
mod image_input_replaced;
mod incremental_construction;
mod incremental_flex_overflow;
mod input_percentage_min_content;
mod mirror;
mod pdf_fidelity;
mod render;
mod table_cell_height;
mod table_cell_width;
mod table_collapsed_borders;
mod table_flex_intrinsic_height;
mod vertical_writing_mode;
