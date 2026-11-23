# Underline and strikethrough at a rich-text caret

Tested source e96c3b985bf6620a9d08ef559eb5327ef74bc7ae against exact integrated parent 9da12d6ebf8fd77b8ee85b34c180bb9875942a6b. The parent executable SHA-256 was 9dd9081a9c39178769331cea59a328ebd4fa4bd98b514a50dcd9f573318d02f7. The candidate executable SHA-256 was 8625e0d6b6ce7438edf3aa44e01cd3748e94a50c61186077a983edb60ca96e9a. Each build receipt records source, Git tree, compiler, profile and pinned JSC archive.

The commands now set the decoration lines for the next rich-text insertion. Underline and line-through can coexist. The command itself leaves the DOM and undo history intact at the caret. On typing, the editor can split a semantic carrier or an inline declaration around the inserted text, preserve other decoration lines, and apply a semantic element or a CSS span according to styleWithCSS. A stylesheet or link decoration can remain effective after the pending decoration is toggled off.

## Matched WPT results

| Cohort | Pages | Shared names | Parent passes | Candidate passes | Gains passing Chrome | Other gains | Losses |
|---|---:|---:|---:|---:|---:|---:|---:|
| Formatting | 44 | 33,316 | 29,480 | 29,496 | 16 | 0 | 0 |
| Multitest | 10 | 10,092 | 8,897 | 8,992 | 83 | 12 | 0 |

Independent QA repeated the candidate and confirmed 111 gains and zero losses across 43,408 matched names. All 916 parent passes that fail signed Chrome 158.0.8127.37 were retained (766 formatting and 150 multitest). The 77 command-return gains and 34 state-query gains are enumerated in changed-names.json. The twelve gains that fail Chrome concern state after forwardDelete, indent or justifyLeft. Fully passing pages stayed at 2/44 and 0/10. The formatting ledger includes another 36 names emitted only by Chrome. Rows contain URL, name, occurrence, parent, candidate and signed reference. Parent/candidate WPT runs were fresh and the signed WPT reference receipts came from the preceding matched profiles.

## Direct controls

Independent QA exercised 108 cases covering pending state, typing, undo, class and inline styles, simultaneous decoration lines, nested italic, plaintext and locked content. Both state flags matched signed Chrome after every toggle in 108/108. Post-typing serialized markup agreement rose from 40 to 84/108, and exact focus path/offset agreement rose from 28 to 84/108. No case that previously matched Chrome's markup, focus or original-node retention became a mismatch. Typing event traces retained 96/108 agreement. Undo restored starting markup in all 108 and original caret endpoints in 96/108. Snapshot-based undo still diverges in retained node identity and event sequences.

Remaining post-typing markup differences were confined to combined semantic carriers (6/12 matched), nested italic carriers (6/12) and locked content (0/12). All other groups matched 12/12. In the 70 additional combination controls, effective decoration lines, text color, font weight/style and vertical alignment at the resulting caret jointly matched Chrome in 70/70, up from 20/70 on the parent. Exact markup matched 41/70, up from 20/70. Differences include CSS shorthand versus longhand serialization and the position of nested formatting elements. These measurements cover CSS and DOM state; they do not measure painted pixels.

The owner additionally measured a 40-case basic matrix (markup 16→40/40) and a 56-case authored-style matrix (18→44/56). All three exact native filters passed, including the new combined-decoration typing and undo guard. The independent and owner direct snapshots are stored separately. In the owner's three archived probe files the snapshot keys sub and sup refer respectively to the results of queryCommandState('underline') and queryCommandState('strikethrough'); the independent records use underline and strike explicitly.
