# Visible-space deletion on the combined non-CSS topic

LP cbdeb1193 includes the merged-aa0268b47 follow-ups and preserves visible whitespace and wrapping when deleting a space. Compared with fadcbb56e, this checkpoint gained 111 named passes and lost none on 2,857 matched non-CSS URLs. Of these, 110 were editing cases reproduced in the separate 710-page editing comparison. One additional XHR timing/status observation changed in the broader run; the editing source does not establish a causal connection. The set of reported names and the 1,492 fully passing LP pages remained the same.

| Browser | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP fadcbb56e | 184,517 | 1,492 |
| LP cbdeb1193 | 184,628 | 1,492 |
| Chrome stable 158.0.8127.37 | 191,970 | 2,323 |

These are 2,857 of the previous 2,859 guard URLs. After an updated common risk audit, two previous Web Audio Chrome hangs were withheld. The original LP shard was stopped after preserving its completed page rows and original server configuration; the continuation ran only the remaining selected pages. Publication pairs both LP receipts with new local Google Chrome stable at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Chrome passed 12,272 names that the candidate missed, and LP passed 4,930 names that Chrome missed. Original names and duplicate occurrences are retained in names.jsonl.gz, with null for absent observations. The browser columns are control, candidate and Chrome. Full inventory coverage is tracked separately.

[Exact Chrome 158 profile, guard receipts and Studio cross-check](https://github.com/povio/pyro/blob/f6149854f6bd023cbfd95e0b03f75c79d509bac8/evidence/round6-chrome158/README.md) · [Editing source, geometry controls and isolated measurements](https://github.com/povio/pyro/tree/f2b878c6330ed7f316d7cefefcac2b7ac6e3ef62/evidence/round6-visible-space-deletion)
