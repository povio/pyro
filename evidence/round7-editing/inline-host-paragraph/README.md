# Paragraph insertion in constrained editing hosts

The accepted runtime is `459fad29dc384ae1242b7bec76f8d23a3fa05a4f`. It uses the caret's whitespace mode for line breaks in hosts that cannot create a child paragraph. A preserved linefeed has a separate text node so the caret can land at the start of the following text, or just before a padding BR. Semantic block hosts can split a block-styled span. A phrasing span host retains a line break inside that child. A select is atomic: Enter at its outer boundary splits the surrounding paragraph; an internal option position resolves to the paragraph start. The original form control and its options move together.

## Complete source patch

`c130cd924-to-459fad29d.patch` is the full product and native test diff from the original parent, covering editing.rs and editing_js.rs. Its SHA-256 is `0fcf9565e563c87bb8b9036d7db5f7c0faf5ff7c1cbf9d414fe32eed13fb8880`. Applying it to a temporary index at `c130cd924` reconstructed the exact tested runtime tree `c3d257cda41592588cf95dde04411aa1eecefe43`. It also applied cleanly to an index at the shared revert `623791975`, producing tree `9aeb4a8c3f5242bbeb382ca48151f330ec58ef7f`. That second result verifies patch applicability only. The runtime measurements below use the original exact base.

## Named outcomes on the 122 primary pages

All revisions and signed Chrome completed all 122 pages with harness status 0 and exactly the same 15,493 URL/name/occurrence keys.

| Source | Named passes | Fully passing pages |
|---|---:|---:|
| Parent c130cd924 | 14,400 | 15 |
| First candidate ad7467f57 | 14,892 | 81 |
| Separate-LF candidate ef248c500 | 14,892 | 81 |
| Held select regression 7a55e1c8e | 14,889 | 63 |
| Accepted 459fad29d | **14,892** | **63** |
| Signed Chrome 158.0.8127.37 | 14,928 | 60 |

| Accepted compared with | Gains | Losses | Gained names also passing Chrome | Lost names passing Chrome |
|---|---:|---:|---:|---:|
| Original parent | 492 | 0 | 450 | 0 |
| ad7467f57 or ef248c500 | 24 | 24 | 24 | 0 |
| Held 7a55e1c8e | 3 | 0 | 3 | 0 |

The initial +492 contained 426 Chrome passes and 66 Chrome failures. The accepted +492 retains every one of those 426 signed gains, adds 24 signed gains on editing/run/insertparagraph.html, and has 42 other gains. Its 24 lost passes relative to ef248c500 occur where the WPT asks a phrasing span host to split a CSS-block span child. Chrome instead inserts a break inside that span; the accepted behavior preserves the signed-reference match on the direct display cases. These losses are relative to the intermediate revision; none had passed on the original parent. The target inline host WPT went from 640/880 to 880/880, and the constrained-container WPT went from 156/420 to 384/420 (Chrome 880/880 and 308/420 respectively).

The following occurrence-1 assertions in editing/other/insertparagraph-in-non-splittable-element.html passed on the exact parent, failed on 7a55e1c8e, and passed on the accepted runtime and Chrome:

* `insertParagraph in select of <div><select><option>abc</option></select></div>`
* `insertParagraph in optgroup of <div><select><optgroup><option>abc</option></optgroup></select></div>`
* `insertParagraph in option of <div><select><option>abc</option></select></div>`

The held runtime cloned option elements and inserted a BR in the control. The accepted runtime keeps the select subtree intact and creates the paragraph outside it. Independent QA confirmed those exact restorations, all original and additional signed gains and the complete 122-page comparison.

## DOM, caret and undo

Independent checks included 32 two-separator cases, 36 display/child cases and 16 select cases. In all 16 select cases the accepted markup, caret, original select and option identity, and selected index immediately after Enter matched signed Chrome. The exact parent matched markup in 6/16 and caret in 4/16; the held runtime matched neither in any case. On undo, **all 16 accepted snapshots were identical to their exact-parent snapshots**: original markup and selected index restored in 16/16, original object identities retained in 0/16, original caret restored in 1/16 and Chrome post-undo caret matched in 0/16. Chrome retained both original objects in 16/16 and also canonicalized the caret. For example, after undo from a point on the select at offset 0, Pyro points inside the first option's text; Chrome points before the select in its containing div. This identity/endpoint limitation predates the change.

The 36 display/child cases had 18/36 exact post-edit HTML matches and 20/36 exact caret matches against Chrome, up from 16 and 2 on the exact parent. The four ordinary span/block-child cases that temporarily diverged were recovered. Across the 32 two-step controls, both serialized edits matched in 16/32 and both carets in 31/32 (parent 10 and 16; first revision 13 and 13). Event traces matched in 16/32. Hosts and outside text survived; undo restored original markup and selection in all 68 cases. DOM node identity after a serialized undo can differ.

Two illustrative Enter sequences begin with `a [] b`. A child path uses zero-based indices relative to the host. Each caret is collapsed.

* **Normal-space span:** parent and final produce `a <br> b` then `a <br><br> b`; Chrome produces `a <br>&nbsp;b` then `a <br><br>&nbsp;b`. All place the caret in the following text at offset 0 (child 2, then child 3). The ordinary-space versus NBSP difference is unchanged.
* **Block-styled span inside a div:** the parent inserted outer div wrappers around each half. After the second Enter its structure was `<div><span>a </span></div><div><br></div><div><span> b</span></div>`. The final candidate and Chrome use sibling spans, then an empty middle span containing a BR. In pre-wrap they agree exactly: `<span>a </span><span> b</span>`, then `<span>a </span><span><br></span><span> b</span>`. In normal whitespace the final candidate keeps ordinary spaces, while Chrome uses `a&nbsp;` and `&nbsp;b`. All spans in this example retain display:block. Parent caret paths are [1,0,0] then [2,0,0]; final and Chrome are [1,0] then [2,0], all at offset 0.

In the separate 72-case builder matrix there were no newly lost Chrome markup matches versus the parent. Three raw caret-path matches were lost for a p host containing an inline-styled div under preserved whitespace. The parent used two outer wrappers and Chrome used one; both pointed to [1,0,0] offset 0. The final candidate follows the pinned WPT's linefeed inside the original inline div, with the caret at [0,2] offset 0. These are not three WPT regressions. Raw markup, paths and direction for every arm are in direct-dom-caret.json.gz.

## Receipts and reproduction

* primary-receipts/ retains the exact raw 122-page JSON for all tested revisions and signed Chrome, losslessly compressed. primary-summary.json records original byte hashes, counts, and profile. all-revisions.jsonl.gz contains URL, literal name, occurrence, parent, ad7467f57, ef248c500, 7a55e1c8e, accepted and signed Chrome status in that order. final-gains.jsonl.gz contains all 492 original-parent gains with per-name Chrome attribution. Each delta-from-* ledger contains URL, name, occurrence, comparator, accepted and Chrome status for every change against that comparator.
* The builder ran the exact base and accepted binary on the 54-page static, control-command, plaintext Enter and rich formatting guards: all 13,060, 4,597, 2,852 and 33,316 Pyro outcomes respectively held. The supplemental select/void/paragraph guard also retained all 389 named outcomes. Guard cohorts overlap and are reported separately. Chrome references for earlier guards are reused from the matching signed static or wptserve/testdriver profile; the extra select/void guard was freshly compared on all three binaries.
* WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The primary used wptserve, testdriver, one worker, and the archived signed 158.0.8127.37 launch-158 wrapper. The original recorded base and Chrome runs were reused for the intermediate and accepted comparisons; the final head ran all 122 pages.
* Original parent `c130cd924cbeefeb07e498626cd9e23a479b27e8` binary SHA-256 `db305e377d1151dcb9864d6445a3d2fffac35db0e9d2325b45663e87dfb7d74a`; accepted `459fad29dc384ae1242b7bec76f8d23a3fa05a4f` binary SHA-256 `4c7ba157321ba69817b2f022011334bb66e77e130bfa6851c4fe9f88b1a76166`. Both used the private wptiter render target and JSC archive SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`. Tracked source was clean; only in-progress evidence files were untracked on the final build.
* Four exact pyro-webapi native integration filters ran one test each and passed, including editing_js::paragraph_insertion_leaves_select_subtrees_intact, editing_js::paragraph_insertion_in_a_phrasing_host_uses_the_caret_style, editing_js::contenteditable_line_breaks_arrows_and_words and editing_js::undo_and_redo_restore_text_controls_and_hosts. The select test checks markup, original select and option identity, selected index and caret.

```sh
gzip -dc all-revisions.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing --server wptserve --testdriver \
  --jobs 1 --tests urls.json --bin "$LABEL=$BIN" --out "$LABEL.json"
cargo test -p pyro-webapi --test integration editing_js::paragraph_insertion_leaves_select_subtrees_intact -- --exact
```
