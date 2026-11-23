# Typing at hyperlink boundaries

Tested source 8500577efcfd9d6b8d5225193a935fd0d75713b7, tree 04390d3ad1c1d8e8f5e9f2bcb4941b8ec18ac562. Source, binary and JSC digests are in pyro-provenance.txt. The combined QA executable for bd247dd67babbff29fb87017f9a6bededfb68ea2 had SHA-256 402050e14f3b7549dcfbc2f1cecfd4b2fe5d6ea79327fec3fe50fc0ffaf4b588. Both builds used the same pinned JSC. The signed reference was Chrome 158.0.8127.37 through round6-reference-158/census/launch-158.

## Named tests

At WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, single-worker wptserve with testdriver completed all 27 pages in each arm with zero restarts. Parent passed 5,674/6,277, candidate 5,873 and Chrome 5,987. Fully passing pages rose from one to seven. Independent QA verified all 200 gains against signed Chrome. Collapsed selections gained 112, noncollapsed selections gained 72, and typing after joining paragraphs gained 16. All three runs had the same named assertions.

The sole pinned loss was occurrence 1 of `[["inserttext","a"]] "<a href=/>foo[]</a>bar" compare innerHTML` on editing/run/inserttext.html?1001-2000. The test expects the typed character inside the anchor. Fresh typing at the trailing edge puts the character outside on the candidate and signed Chrome. This assertion also appears in the 13,060-name static guard cohort. The other 13,059 static checks retained their outcome. Multicommand (10,092), formatting (33,352), Enter (2,852) and control (4,633) guard ledgers had zero changes. Cohorts overlap. Guard parent/head measurements are from this run. Static, formatting, Enter and control Chrome guard receipts were reused.

## Fresh typing and replacement

Independent QA observed two-stroke markup, linked text and caret agreement improve from 6/14 to 14/14 for bare anchors and separately for nested anchors. Editing-host links and block links retained 7/7 each. Empty anchor markup still agreed in 0/14, no-href in 10/14 and nonlink controls in 5/7. All 77 edit event sequences matched. The QA archives retain originals, endpoint paths, event data and undo observations.

In the separate 24-case continuation matrix, parent and candidate gave identical final DOM and endpoints for keyboard typing and scripted insertText. When replacing the last selected character of an editable link, all 18 editable first-step snapshots matched Chrome. In a bare anchor, Pyro produced `<a>abXY</a>` and Chrome produced `<a>abX</a>Y` (href omitted here). The same difference survived recollapsing at the caret or moving away and back before the second key. Continuing the existing undo typing group retains the pinned replacement assertions. The six locked-anchor cases also retained parent behavior and diverged from Chrome, which left the locked text intact. Full markup and endpoints are archived in continuation.json.gz.

In the owner's 48 boundary controls, two-key markup agreement rose from 23 to 36, and in 35 additional paragraph controls it rose from 5 to 25. Link connectivity immediately following the edits agreed with Chrome in all 83. Undo observations in the archives include cases where Pyro reconstructs the original markup with a new link node.
