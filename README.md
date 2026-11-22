# Round 7 shared fix ledger (single source, both machines)

`ledger.jsonl` has one JSON object per line. Its `kind` is `brief`, `ownership` or `policy`.

States: `proposed` (evidence and suspected cause only), `validated` (failure confirmed from census receipts; touched code checked unchanged on current source; cause hypothesis may still need a native repro), `claimed` (owner named, not yet editing), `active` (editing), `frozen` (source SHA fixed, awaiting or in build), `reservation` (corpus-run reservation, NOT product ownership), `excluded`.

To claim: edit the brief's `state` and `claimed_by` and push, or message the MacBook queue researcher.

Evidence: census git 5892e136, evidence/rc5-census-516dc3159-combined/asymmetric/chrome-only-confirmed-names.jsonl.gz (75,805 rows / 10,945 URLs). Source checked against integration/round7-local fabdc9fb0 on 2026-11-22. No browser runs were made for this ledger.

Rules: one brief = one root cause and one touched-symbol set. No security, crash or held work. Corpus reservations are not ownership.
