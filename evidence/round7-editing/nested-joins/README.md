# Joining nested paragraphs

Tested bd616707859370498b431b0e333c072ea0706f89 against the exact integrated parent d89a9b5704599be80cbb42f7cc320d92c6f86c7a. The first candidate was 3b3e3a3c0cbec0ef091db39a6ef5bde9205df01b. The private executables, clean source trees, compiler and pinned JSC digests are recorded in the build receipts.

The change handles a single nested editable block paragraph beside an anonymous text or placeholder line. It resolves the deletion boundary before dispatching beforeinput, so cancellation precedes mutation. The destination retains its text node for a text-to-text join. An empty source contributes no text. When the surviving paragraph is promoted out of a redundant outer DIV, whitespace repair runs before promotion while the original computed style is available. Attributed outer containers and neighboring blocks are retained. Styled source content that requires its own formatting carrier continues through the existing editing path.

## Named results

Independent QA measured 12,797 → 12,821 passes across the same 13,060 named assertions, with no losses or membership changes. Each of the two chrome-compat WPT pages rose from 6/18 to 18/18. All original sibling cases remained passing. All 24 newly passing nested cases pass signed Chrome 158. The first candidate recovered 16; the final candidate added eight and retained all 16. Fully passing pages rose from 43 to 45 of 54. These compatibility fixtures document browser behavior.

Independent QA retained 426/426 preformatted join assertions across all nine pages. The owner additionally compared nine input-event target-range pages. All 2,445 statuses were identical across both Pyro candidates and the parent (1,797 passes each). The two supplemental WPT reference selections were drawn by URL from the signed testdriver receipt. Static and supplemental profiles are recorded separately.

## Direct DOM, caret and undo

Independent QA verified 28 nested DIV cases covering both directions and nesting orders, placeholders, neighboring paragraphs, locked/styled content and preserved whitespace. Exact post-edit markup agreement with Chrome rose from 3/28 on the parent to 17/28 on the first candidate and 23/28 on the final candidate. The final six improvements were all nested-first. Exact post-edit caret agreement rose from 10 to 25/28. The five remaining markup differences were forward Delete with a locked inner-first paragraph, and both directions for styled or preformatted inner-last cases. The separate 16 section/p probes retained their parent observations on the first candidate; these were distinct from the 28 DIV cases.

Undo restored the starting markup in all 28 cases and the original caret in 26/28 on the parent and both candidates. The post-undo caret agreed with Chrome in 3/28 on both parent and final: Chrome itself often restores a different equivalent position.

One newly effective edit exposes a history limitation. For forward Delete before the BR in an outer paragraph that starts with a placeholder and contains a nested paragraph, the parent did nothing and kept the original nodes. The candidate removes the BR, enables undo, and matches Chrome's post-edit DOM, caret and node retention. Undo restores the original markup and starting caret, but Pyro reconstructs the nodes from its snapshot. Chrome restores the original node objects. The archived observations distinguish identity from markup.

QA reran the final guard after correcting an output-path collision and checked the executable paths for both final WPT receipts. The first-candidate incremental comparison reused the owner's archived WPT output. The full directional ledgers, direct observations, page statuses and per-build hashes are adjacent.
