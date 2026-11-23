# Selected fontSize on FONT and SPAN

Tested source `bdd545a4da6bbdfe3f7b8ae59ba03667c96a32da`, tree `96a05a460822b0c8abd8acae8af9aab266add9d8`. Executable SHA-256 `269e5368190332fe6405ba900dbdf4a758e1bdd2b00d1535d17e32df533ff23d`. Integration parent `25d3ce49d` differs from tested `709c67336` solely in evidence. The rehashed foreColor executable and final named ledger supplied the baseline; the original build receipt is included. Signed comparator was Chrome 158.0.8127.37.

Independent QA matched 33,316 named assertions on 44 pages with unchanged membership. All 22 gains are signed passes among the 25 frozen targets. Two previous pinned passes became failures. On both exact fixtures, signed Chrome and candidate produced identical actual markup in forward and backward selections. The partial CSS large SPAN becomes two surrounding SPANs and a FONT size=4 for selected text. The spanning legacy FONT size=4 extends through the selected portion of the following large SPAN. Both losses are counted. [Per-name results](target-results.json)

Three signed targets remain open. In both modes nested 2em within FONT size=6 produces different markup and neighboring sizes. Chrome splits the outer FONT around selected content. The candidate retains it. The legacy cross-FONT queryCommandIndeterm after applying size 4 reports true in WPT, which expects false and passes in Chrome. In the isolated direct probe the candidate and Chrome both return false. The WPT failure remains counted.

Core first-command HTML matches in 94/96, rendered sizes in 96/96, backgrounds in 94/96 and both endpoints in 87/96. Neighboring sizes match in 56/72, backgrounds in 72/72. The correction recovered the six core markup and seven endpoint regressions, along with both nested neighbor cases. After two undos all 168 selections matched Chrome on endpoints.

## Undo attribution

The single-command probe measured 80 actual DOM edits with undo enabled and returning true. The remaining 16 commands kept the original DOM and returned true from fontSize, with undo disabled and returning false; Chrome enabled undo and returned true. Across the 80 edits the candidate disconnected original tracked nodes on undo; Chrome retained them. Original connectivity matched Chrome on the 16 unchanged-DOM cases.

The parent-relative final undo identity losses are a separate set of 16. The sets have **zero fixture IDs in common**. Each set covers full and partial selections in both CSS modes and both directions.

- **Final identity losses** cover span-large and span-px. On those CSS large and 18px SPAN fixtures the parent skipped the edit and returned false from undo. Chrome and candidate normalized the markup and returned true from undo. The candidate’s restored host disconnects the original carrier, text and neighbor. The eight intermediate losses are a subset of these 16 final losses.
- **Unchanged DOM without undo** covers font-large and font-styled. Each original FONT already has size=4.

[Stable fixture IDs, intersection and both differences](history-cohorts.json) · [Independent reconciliation](independent/revision2/undo-fixture-reconciliation.json)

The selected font-size native regression test passed. Fresh final candidate results kept all 10,092 command multitest and 13,060 static guard statuses from the verified parent.
