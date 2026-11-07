# Pagination plan for cmr-overflow: closing and passing Chrome's speed

All percentages are shares of browser-thread samples on cmr-overflow (17,572 total). Chrome is 279 ms against our 354 ms, so matching it needs about 21% off; passing it needs more. Assuming sample share tracks wall time, items 1 to 7 together remove about 28%, which would put cmr near 255 ms. Adding items 8 and 9 brings it to about 35%, near 230 ms.

Two things to check before relying on these numbers:
- **Does the 20% sheets-key cost show up on the other documents?** It only happens when an animation or transition listener has been registered, because that flag never resets (events.rs:196). Re-profile booking-request and house-bl to see whether they pay it too.
- **The largest cost is still untouched.** The measuring container's own relayout (flex 23.8%, block 23.3%, table track sizing about 18%) is in none of these items. It is the next thing to investigate after this plan lands.

## Ranked plan

### 1. Compute the timing rules lazily in `observe`, then replace the sheet-text hash with a revision counter
- **Mechanism, in two steps:**
  - **Step 1a (do first, S effort).** Move `state.rules()` (lp-webapi animation_events.rs:265) to after the early returns in `style_change`, which are at lines 283, 286, 293 and 295. The rules are only used at line 296, and the profile shows that line is never reached. Paprize's clones are always first observations. This alone removes about 19%.
  - **Step 1b (S/M effort).** Add a per-Dom counter, `style_text_revision`, plus a per-owner revision stored on the element's node data. Do not key the per-owner revision by u32 slot, because slots get reused.
    - **Where to bump:** in `note()`/`note_more()` (dom.rs:422-434), before the `journal.last_mut()` match, because a journal overflow clears the journal. Bump when the changed node's parent is an HTML `<style>`. Do not bump on the generic `Change::Unknown` that `bump()` pushes first, since every mutation starts that way. Instead, bump explicitly at the two places that never refine it: the document reset (dom.rs:775) and attachShadow (dom.rs:986).
    - **New key:** `author_sheets_key` (style.rs:1003-1028) becomes (generation, style_text_revision, a bitset of which sheets are connected). Delete `sheets_key` and `hash_text_content`. `sheet_revision` returns the per-owner revision.
    - **Semantics:** today four places read descendant text. Switch all four to child text content in the same change (the HTML spec, Blink and Gecko all use child text): `hash_text_content`, `for_each_author_sheet` (style.rs:998), `render_style_text` (snapshot.rs:661) and the `style_owner` walk in `render_delta` (snapshot.rs:527-538). Switching only some of them breaks things.
    - **Why 1b is needed after 1a:** 1a leaves elements whose style changed still paying the hash, and `update()` still pays it once per frame.
- **Files:** lp-webapi animation_events.rs:88-106, 248-296; lp-dom style.rs:770-789, 991-1046, 1451-1461; dom.rs:397-434; snapshot.rs:527-538, 646-664.
- **Realistic saving:** about 19.5% (`hash_text_content` is 3,565 samples).
- **Effort:** S for 1a, M for 1b.
- **Main risk:** a `<style>` edit that misses the counter leaves animation and gecko-only rules stale within a task. The `StyleEngine` only sees `<style>` edits at the task checkpoint (lp-browser lib.rs:574), so the counter has to cover synchronous edits. A further risk is the behaviour change when an element is nested inside a `<style>`.
- **Test:**
  - lp-dom unit tests: the counter moves on `textContent=`, `appendChild(text)`, `.data`/`appendData`, a streaming parser chunk that splits a `<style>` body (parse.rs:102), and moving or removing a child text node. It stays still across 1,000 non-style appends and attribute changes.
  - In debug builds, assert that the old hash changes only when the new key changes.
  - JS tests: in one task, set a `<style>`'s `textContent` to add `@keyframes`, then call `getComputedStyle`; `animationstart` fires. The first read of a new element fires nothing; a second read after a class change fires `transitionrun`.
  - WPT in CI only: css-animations, css-transitions, the-style-element.

### 2. Keep a count of sticky elements on Blitz's `NodeTree`, replacing the scan behind `Geometry::has_sticky`
- **Mechanism:** add an `AtomicUsize` to `NodeTree`, next to `geometry_generation` (tree.rs:51). Do not use a `NodeFlags` bit: the flags field is not atomic, and the parallel traversal only holds `&Node`. At each place that writes a primary style, compare the stored position before and after the write and adjust the counter:
  - `RecalcStyle::process_preorder` around `recalc_style_at` (stylo.rs:1355-1366);
  - `sync_pseudo_element_styles` (stylo.rs:1489);
  - pseudo-element creation (construct.rs:744, 797-805);
  - `clear_data` (stylo.rs:770): decrement if the style was sticky;
  - node drop, in the body of `drop_node_ignoring_parent_with` (document.rs:1025-1051), including pseudos and anonymous blocks.

  Anonymous-box writers (construct.rs:163, damage.rs:239) can never produce a sticky style, because `position` is not inherited; record that in a comment. `Renderer::has_sticky` (lib.rs:1163) then reads the counter. Delete the `Cell` (geometry.rs:103, 117, 145-154).
- **Realistic saving:** about 5.5% (995 self samples at lp-render lib.rs:1165).
- **Effort:** S.
- **Main risk:** a missed writer gives a false negative, and `sticky_shift` then silently produces wrong rects.
- **Test:**
  - In debug builds, assert the counter matches the old scan inside `Renderer::has_sticky`.
  - lp-render delta tests: a class switches an element to sticky and back; a sticky element sits under an ancestor toggled to `display:none`; the sticky element is removed; `::before{position:sticky}`; two sticky elements, one removed.

### 3. Index the `::before`/`::after` rules that can generate a box, and skip the eager-pseudo match and cascade for everything else
- **Mechanism:** in `CascadeData::add_styles` (stylist.rs:~4026), build a second `SelectorMap` per origin. It holds only single-level Before/After rules whose declaration block declares `content` or `animation-name` (checked on the longhand set, which covers `all`, `var()`, `inherit` and Tailwind's `content: var(--tw-content)`). Clear it wherever `normal_rules` is cleared. In `match_pseudo` (style_resolver.rs:552-614), when `pseudo_resolution != Force` and no generating rule matches in any applicable `CascadeData`, return `None`. Fall back to the full path when shadow, slotted or part data has before/after rules. Do not add a FirstLetter/Selection boolean: `rules()` already returns `None` for those.
- **Realistic saving:** about 5%, out of 1,100 samples spent at style_resolver.rs:309.
- **Effort:** M.
- **Main risk:** the index going stale on a CSSOM declaration edit. Today that is safe because `StyleRuleDeclarations` triggers `FullyInvalid` (stylesheet_set.rs:436-438); add a test that fails if that is ever narrowed. The other risk is the `Force` path.
- **Test:**
  - `*::before{box-sizing}` produces no pseudo style, and `getComputedStyle(el,'::before').content` is unchanged.
  - `.x::before{content:''}`, `img::before` and `q:before` still generate.
  - A CSSOM edit `cssRules[i].style.content='"a"'` makes the pseudo appear.
  - `.a:last-child::after{content:''}` toggles on append and remove.
  - `LP_GEOMETRY_PROFILE` styled/matched counts are unchanged.
  - WPT css-pseudo in CI.

### 4. Skip the grid's min-content recheck for items whose inline contribution was never used
- **Mechanism:** in pass 1, record on each `GridItem` whether its inline contribution was consumed. That includes measured content, max-content use, fr sizing under an indefinite width, and any row-dependent read such as an aspect-ratio transfer from a percentage block size. The recheck at grid/mod.rs:518-541 skips unflagged items, so `Some(new) != None` stops forcing a rerun every time. Skipped items still have their caches reset, in case another item triggers the rerun.
- **Realistic saving:** about 1.5% (278 samples at mod.rs:520, plus 32 for the rerun).
- **Effort:** S.
- **Main risk:** missing a consumption path, which leaves columns stale.
- **Test:** Taffy fixtures. An fr column with a `min-width:0` item shows zero recheck measures and an identical layout. An auto column with `min-width:auto` still reruns. WPT css-grid and css-sizing in CI.

### 5. One sync per `getBoundingClientRect`
- **Mechanism:** in `bounding_client_rect`, `client_rect` and `box_quads` (lp-dom layout.rs:673-678, 851-866), call `synced_geometry_source` before `is_visible` when a geometry source is active. Its `engine_synced` call stamps the engine, so `engine_styled` then returns early at style.rs:858. With no geometry source, keep the current order.
- **Realistic saving:** about 0.7% (the second pass's fixed shell).
- **Effort:** S.
- **Main risk:** with size container queries, `is_hidden` may now see post-layout styles.
- **Test:** count syncs with `Layout::syncs`: one append followed by one rect read gives exactly one sync. Cover visible, `display:none`, `visibility:hidden` and detached elements, plus a container query that toggles `display`.

### 6. Parse the base URL when it is set
- **Mechanism:** store `(String, Option<Url>)` in the constructor (lp-layout lib.rs:135, parsing BLANK too) and in `set_base_url` (lib.rs:491). `render_opts` (lib.rs:236-254) borrows the parsed form. `Snapshot.base_url` keeps the original String, because `Url` normalizes. Moving the parse into the `stylesheet_href` closure would work equally well.
- **Realistic saving:** about 0.5% (103 samples).
- **Effort:** S.
- **Main risk:** none significant.
- **Test:** existing tests for linked sheets and `<base>` handling; check that navigation still updates the base.

### 7. Turn off the always-on timers unless profiling is requested
- **Mechanism:** use a runtime `AtomicBool`, set by `LP.geometryStats` or `Performance.enable`. Do not use the `OnceLock` env var (lib.rs:65). `bench/pdf/run.mjs` and `Performance.getMetrics` (lp-cdp engine/mod.rs:971-980) read these timers, so they must keep working when enabled. Vendored blitz `phase_profile::mark` (resolve.rs:761-777) needs its own gate. Keep the timers that drive behaviour: `started` (lp-render lib.rs:989/1002) and the first-layout deadline at net.rs:162. Move container_queries.rs:139-243 off `blitz_phases()[4]` and onto `blitz_passes()`/`geometry_generation`.
- **Realistic saving:** about 0.5%.
- **Effort:** S.
- **Main risk:** metrics silently reading zero.
- **Test:** `LP.geometryStats` returns non-zero once enabled; the fetch-timeout tests pass.

### 8. Split the damage bits and stop construction damage at the box that owns it (enabler)
- **Mechanism:** this merges the layout-incremental and tables-text proposals.
  - The mutator currently sets `ALL_DAMAGE` on the parent (mutator.rs:213/223, 567, 606, 806). Replace that with a separate "children changed" bit, `CONSTRUCT_FC`, so it can be told apart from construction damage that comes from the node's own style.
  - In `propagate_damage_flags` (damage.rs:~107-112), generalise the `REORDER_CHILDREN` rule so that a node whose children changed forwards only `RELAYOUT|CONSTRUCT_DESCENDENT`.
  - Keep forwarding construction damage in these cases:
    - non-atomic inline boxes, up to their inline root;
    - `display:contents`, to the first ancestor that is not `contents`;
    - any construction damage on a row or row group, to the table root (rows are never visited as layout children);
    - list items, to the nearest `ol`/`ul` (list.rs:17-40);
    - a change to a box's own display or position, to its parent.
  - Also fix an existing bug: a `RELAYOUT` that comes from style on a table-internal box must rebuild the table context, because width, padding and border are copied into `TableCell` today.
- **Realistic saving:** about 2% on its own (about 136 samples of ancestor re-collection, plus table rebuilds for changes inside cells). It also unlocks items 9 and 18.
- **Effort:** M.
- **Main risk:** a predicate that misses a case leaves the anonymous-block partition or the `TableContext` stale, and nothing reports it.
- **Test:**
  - Equivalence test: incremental layout versus `mark_all_damaged` from scratch, comparing `final_layout`. Cases: text inside a `td`; a `tr` or `td` inserted; an abspos child added to a `tr`; an `li` inserted inside a `div` inside an `ol`; a `span` switching to `display:block`; `display:contents`; `::before` content; a stylesheet change to `td` width.
  - Debug counters for `build_table_context` and `collect_layout_children` calls per resolve.

### 9. Incremental line breaking: resume from the first dirty line (depends on item 8)
- **Mechanism:**
  - `set_node_text` records `dirty_from`, the common-prefix length it already scans.
  - Stop dropping the inline layout at construct.rs:438, and keep the old lines through the `BuildCache` `clone_from` (builder.rs:358-372) and `BreakLines::new`.
  - Add parley `BreakLines::resume(from_line)`, using the `BreakerState` saved at each line start.
  - Resume one break opportunity before the old end of text, to handle hanging whitespace and the final mandatory break.
  - Validate the prefix with a style-change bit taken from Stylo's real damage (needs item 8), or by comparing advances. Comparing run boundaries is not enough.
  - Fall back to a full break for: a width, indent or inline-box size change, floats, bidi or `dir=auto`, `justify`, `hyphens`, `::first-line`, and font fallback changes.
  - This also lets the inline root keep its own layout across construct, so the `content_widths` recompute (209 samples) can be skipped when the IFC is untouched.
- **Realistic saving:** about 5% (`break_next` is 1,157 samples).
- **Effort:** L.
- **Main risk:** a stale prefix after a style change that keeps the same run boundaries.
- **Test:** a differential parley/blitz test: random append, truncate and insert sequences give lines bit-identical to a from-scratch break. Corpus: pre-wrap with `overflow-wrap:anywhere`, normal whitespace, justify, `text-indent`, mixed fonts, emoji fallback, and RTL (which must take the fallback). Add a counter for clusters visited per forced layout.

### 10. Stop cloning a whole `ComputedValues` for table cells on every style read
- **Mechanism:** `for_table_cell_layout` (blitz stylo_data.rs:141-175, called at node.rs:1161) does `Box::new(clone)` for cells with a numeric height or collapsed borders, on every child-style read. Replace that with a small override struct in the cell's `layout_data` (height forced to auto, collapsed half-borders), written when table layout computes `collapsed_border`, and read by the size and border accessors.
- **Realistic saving:** up to about 1.3% (174 samples at node.rs:1161 plus 54 at 1189).
- **Effort:** S/M.
- **Main risk:** an accessor that does not consult the override.
- **Test:** table equivalence tests covering collapsed borders and fixed-height cells; WPT css-tables in CI.

### 11. Treat out-of-flow boxes as relayout boundaries, for the root containing block only
- **Mechanism:** add a `POS_CHILD_RELAYOUT` damage bit. A dirty abspos box whose own position did not change returns this bit to its parent instead of `RELAYOUT`, so ancestors keep their Taffy caches. The root in-flow layout then hits the cache, and the root positioning pass (mod.rs:164-192) relays only the dirty candidate. Leave the anonymous-ancestor invalidation walk (damage.rs:165-173) untouched for this bit. The root positioning pass must still fold the relaid box's overflow into the root's overflow. Skip the split for non-root containing blocks; it saves nothing here.
- **Realistic saving:** about 1.5% (392 samples at mod.rs:142).
- **Effort:** S/M.
- **Main risk:** the root's scrollable overflow, and container-query ancestors being wrongly treated as resized.
- **Test:** equivalence tests for abspos at the root with set insets, auto insets, nested abspos, and a change to its own width or inset. A counter of in-flow runs on ancestors should be 0.

### 12. Hand the table's own cell placement to the grid
- **Mechanism:** add a `LayoutGridContainer` hook returning `resolved_placement(child)` and `track_counts()`, answered from the row and column data `TableCell` already has (`ColumnCursor`, table.rs:711-760). The grid then skips its size estimate, named-line resolution, auto-placement and occupancy matrix. Keep the order of `oof_children` and pseudo-elements (mod.rs:720-737). Do not build the per-row height idea (see dropped list).
- **Realistic saving:** about 1.2% (about 270 samples).
- **Effort:** M.
- **Main risk:** a mismatch with taffy's dense placement around rowspan holes.
- **Test:** in debug builds, assert the hook's placement equals taffy's over a table corpus (rowspan and colspan holes, thead/tfoot, `col span`).

### 13. Share styles with DOM neighbours across forced layouts (measure first)
- **Mechanism:** when the per-traversal LRU misses (traversal.rs:524), try the previous element sibling, and the cousin at the same index when the parent shares its style `Arc`, through the existing `test_candidate`. Only for `Sequential` threading. A candidate needs an empty hint, no snapshot, and no pending dirty descendants. Do not require "not restyled this traversal": `WAS_RESTYLED` is never cleared in this build, so that check would reject most candidates.
- **Realistic saving:** about 1 to 2%.
- **Effort:** M.
- **Main risk:** a low hit rate. If the page containers carry `HAS_SLOW_SELECTOR*`, every sibling is restyled on each append anyway, which is a bigger target in its own right.
- **Test:** check `LP_GEOMETRY_PROFILE` shared/matched counts before building anything. Then: rows appended one at a time with `nth-child(odd)`, `last-child`, `+` and `[data-x]` rules all compare equal with sharing disabled. Byte-identical PDFs on the ToS fixtures.

### 14. Build the paint tree when something paints or hit-tests, not on every resolve
- **Mechanism:**
  - Add a `PAINT_TREE_DIRTY` bit, set inside `clear_damage_and_dirty_flags` (damage.rs:295) when the propagated damage has `REBUILD_STACKING_CONTEXT`. Nodes with no damage data stay "always dirty".
  - Add `ensure_paint_tree()` and call it from:
    - the blitz-paint entry points `paint_scene`, `paint_scene_with`, `paint_scene_for_print` and `paint_scene_recording_text`;
    - lp-render `elements_from_point`;
    - hit-testing, i.e. `element_from_point` and `hit`, which currently take `&self` and need `&mut` plumbing.
  - Run `restore_out_of_flow_stacking_owners` inside it.
- **Realistic saving:** about 0.7%.
- **Effort:** M.
- **Main risk:** a consumer that does not call `ensure_paint_tree()` gets a wrong `elementFromPoint` result.
- **Test:** in debug builds, assert the lazily built tree equals a fresh build; `elementFromPoint` after mutations with no paint in between; existing screenshot and PDF tests.

### 15. Answer `is_visible` for rect queries from Blitz box presence
- **Mechanism:** `bounding_client_rect` only needs `in_display_none`. Answer it from whether Blitz has a box or primary style for the element, instead of re-deriving `ComputedStyle` up the ancestor chain (style.rs:882-899). The full restyle-time derivation was judged L effort with a broken trigger, so do this narrower version.
- **Realistic saving:** up to about 0.8% of the roughly 1.7% spent re-deriving.
- **Effort:** S/M.
- **Main risk:** shadow or flat-tree parents, and elements styled before an ancestor became `display:none`.
- **Test:** rects for elements with a `display:none` ancestor, inside a shadow host, and detached.

### 16. Mirror cleanup
- **Mechanism:** delete `Entry.hash` and `hash_kind` (mirror.rs:110-135, 514, 524) and rely on the attribute diff `update()` already does. Make the Text branch compare against Blitz's current text, which `set_node_text` already does at mutator.rs:208. Have `reorder` walk Blitz's child list in place instead of `to_vec()` (mirror.rs:685).
- **Realistic saving:** about 0.3%.
- **Effort:** S.
- **Main risk:** none significant.
- **Test:** the delta-versus-full equivalence tests in lp-render tests/mirror.rs; text set and then reset gives no restyle.

### 17. Narrow some restyle hints
- **Mechanism:** (b) skip `restyle_subtree` on a move within the same parent (mutator.rs:833-838). (a) In `reorder`, remove entries first, then place, keeping the longest increasing subsequence in place. Do the exact-edge half of (c): restyle only elements whose first/last status actually changed, and nothing when only text children changed.
- **Realistic saving:** about 0.4%, unproven.
- **Effort:** M.
- **Main risk:** damage to the moved element inside an inline formatting context.
- **Test:** add a counter of restyles of already-styled elements; mutator tests for `:first-child`, `:last-child`, `:nth-child`, `+` and `~`.

### 18. Incremental `TableContext` on row append and removal (after item 8)
- **Mechanism:** keep `ColumnCursor` and pending rowspans inside the context; push or truncate the last row's cells when that is the only change. Needs a journal record of the structural change, an arena that does not move for `calc_values` (raw pointers), and handling for tfoot ordering, a growing column count, and the collapsed bottom edge.
- **Realistic saving:** about 0.3%.
- **Effort:** M.
- **Main risk:** the edge cases just listed.
- **Test:** a differential test against a fresh `build_table_context`.

## Existing caches on this path and their structural replacements

| Cache | Replacement |
|---|---|
| `StyleEngine::sheets_key` Cell (style.rs:778, 1007-1027) | Item 1b: (generation, `style_text_revision`, connected bitset). Delete it. |
| `sheet_revision` content hash (style.rs:1031-1046) | Per-owner revision on the `<style>` node, bumped in `note()`. |
| `State::rules` / `rules_stamp` (animation_events.rs:64-106) and `gecko_rules` (style.rs:772, 1270-1283) | Parse eagerly in `add_sheet`/`set_sheet_text` and on lp-dom `<style>` text mutation, keeping media conditions unevaluated, and share unchanged per-sheet lists by `Rc`. Low priority (0.2%). |
| `StyleMemo` memo/memo_at (style.rs:760-763, 882-899) | Item 15 for the rect path. In general, derive at restyle time with top-down propagation of the ancestor-derived bits and an `inert` attribute hook. |
| `Geometry::has_sticky` Cell (geometry.rs:103, 145-154) | Item 2's atomic counter. |
| `Geometry::transforms` map (geometry.rs:102, 157-183) | Delete the map and walk `containing_block()` directly, O(depth). Do not add a stored per-node transform. |
| `Geometry::layout_children` index (geometry.rs:107, 122-140) | Separate membership flags for `layout_children` and hoisted lists, or drop the hoisted half. On the reconstruct branch (resolve.rs:552), take the old list and clear its flags before overwriting. |
| `Layout::result` (lp-dom layout.rs:245, 276-292) | Produce it from the sync that changes it, or answer per box. Not hot. |
| Mirror `Entry.hash` (mirror.rs:86, 110-133) | Item 16. |
| `HoistedPaintChild::position` / `hoisted_content_bbox` (paint_tree.rs:116-170) | Compute inside `ensure_paint_tree` (item 14). |
| parley `BuildCache` (build_cache.rs) and `clear_build_cache` (damage.rs:485) | Keep the node's own layout plus `dirty_from` incremental shaping (item 9), and limited sharing by `cloneNode` identity. Do not delete it until hits are measured by origin (clone, same node, rollback, other); removing it now could be a net regression. |
| parley_engine `GlyphCache` (shape/cache.rs:251) | Reshape only from the dirty offset, keeping prefix glyphs on the node. |
| `TextLayout::last_break` hash (text.rs:44, inline.rs:739-756) | Item 9's resume state. Some comparison of width, indent and inline boxes remains unavoidable. |
| `TextLayout::content_widths` Option (text.rs:27, 52-56) | Dead code: nothing reads it. Delete the field, the accessor, and the clears at node.rs:243 and resolve.rs:651. |
| Taffy per-node cache, Stylo sharing LRU, `RuleCache` | Upstream, per algorithm run or per traversal, so they cannot go stale. Leave them. Precise damage (items 8 and 11) reduces how often the Taffy cache is wiped. |
| `PdfCache` (pdf.rs:89-145) | Not on the pagination path. Cross-document reuse of system fonts is deliberate. Leave it. |

## Refuted or dropped
- **Per-sheet parsed timing/gecko rules as a performance item:** worth 0.2%, and as specified it was a cache that went stale on viewport changes. Kept only as the low-priority hygiene in the cache table.
- **Positional edit script built from the journal, dense mirror index, shared attribute storage:** journal entries carry bare slots, so slot reuse turns a reference into the wrong node. The dense index also collides with `SYNTHETIC_TEXT` keys. Real saving is about 0.5%, and the safe part is already item 16.
- **Clone taking its source's style by `Arc`:** unsound, because the parent styles are different objects and ancestor selectors can match differently.
- **Laying out auto-height abspos boxes in one pass:** the doubling comes from flex measuring its items, not from the abspos code. About 0.8% is removable, and the gate needs a known width that Paprize's fit-content container does not have.
- **Replaced/button classification flags in `layout_style`:** about 0.25%. Redirected to item 10.
- **Replacing the inline-layout memos with build-time widths and `broken_for_*` fields:** build-time widths are wrong because inline-box sizes depend on the sizing mode, and the fields are the same memo without the hash.
- **`BuildCache` replaced by `Arc<BuiltParagraph>` plus a revision counter:** loses rollback, turns hits on equal content into full shaping, and puts per-node brushes in the shared `Arc`.
- **Per-row table block sizes:** a cache, thrown away on every `TableContext` rebuild, and duplicating Taffy's cache. Only the placement hook survives, as item 12.
- **Storing transforms from `resolve_transforms`:** that function unions overflow bottom-up and skips undamaged subtrees, so stored transforms go stale when siblings shift; scroll offsets also break it.
- **printToPDF passes:** 0% of the profile. Stylo's servo build does not parse `@page` `size` or `page`, and the restore relayout runs only for documents without `.pz-page`.
- **Non-root containing-block split of the out-of-flow boundary:** 0% here, because all out-of-flow work happens at the root.
- **Restyle hint (d), relying only on the attribute snapshot:** breaks presentational-hint attributes, `lang`/`dir` inheritance, and the lp state attribute.
- **Restyle hint (c)'s `EDGE_CHILD_NON_SUBJECT` flag:** needs changes to selectors-0.41, which is not vendored. Too little gain to justify it.
- **Content-widths part (b), computed eagerly or incrementally:** adds O(L) work to every rebuild, has no append API to hook into, and is wrong at the join between old and new text.
- **Calling `has_sticky` once per query in lp-dom:** saves essentially nothing.

One minor issue spotted along the way: lp-layout lib.rs:348-349 increments `TOTAL_CALLS` twice per delta sync.
## After the plan landed (2026-11-07)
- **Item 3 did nothing until `eed50edb`:** lp-render's UA sheet `*, *::before, *::after { animation: none !important }` declares `animation-name`, so the index filed a rule matching every element (10,000 of 10,000 checks hit). `content: normal | none` and `animation-name: none` now keep a rule out. `match_pseudo` went from 26% to 2% of style time on cmr-overflow.
- **Table rebuilds are not redundant:** the 29 "same rows" rebuilds each had one more cell than the last (Paprize appends cells one at a time); `collect_table_cells` is 2.3% on house-bl-final-overflowing-table and below the cutoff on booking-request-overflow. Nothing to gain from an incremental `TableContext`.
- **`:has()` invalidation (`36944d7c`):** Blitz's own anchor walk restyled every anchor's whole subtree on every insertion; `.cmr-pdf-sheet:has(.cmr-pdf-top)` restyled the page for each appended row, 4× the elements the appended rows restyle. Stylo's `RelativeSelectorInvalidator` now runs on insertion and removal, as in Gecko. cmr-overflow: 10,352 → 3,483 elements styled per render, style phase 98 → 30 ms.
- **The appended clones carry styles:** every appended subtree on booking (13,500 of 13,500) is a `cloneNode(true)` of a styled original and gets `restyle_subtree`; that restyle is inherent (the parent changed), as in Chrome.
- **Sharing with styled neighbours, tried and dropped:** on a sharing-cache miss, trying the two previous element siblings and the first two children of the parent's previous sibling as candidates cut matches per render by only 3–9% (booking 1,399 → 1,355, house-bl 1,745 → 1,584, cmr 1,530 → 1,459). The elements still matched have distinct Tailwind class sets, which no neighbour can pass `have_same_class` for. Not worth a Stylo patch.
- **Where the time is now** (per render, Blitz phases, ms): cmr-overflow style 9, construct 2+23, layout 56; booking-request-overflow style 11, construct 8, layout 13 of a ~100 ms render; house-bl style 14, layout 11 of ~82 ms. Layout and the work outside Blitz (JS, the sync, geometry reads) are what is left.
