# Independent Studio round 7 comparisons

Evidence base: `7f399bf8`. Batch 3 source: `8f6a67d823d9327fc00f8901662d160cf6bd8ed0`. Batch 4 source: `c56b9f5e3fda07c29a9d6ca4c13a104a0e4265dd`. The local candidate binaries match their archived SHA-256 receipts. Frozen and Chrome outcome archives match the frozen Studio cache byte for byte. See `frozen/provenance.md` for Chrome and build provenance.

The 41-URL supplements for both heads are complete and identical to batch 1 in every keyed outcome. Each has 27 strict pages, 577 distinct named passes, 874 distinct names and 41 complete/OK harnesses. Chrome alone passes 8 more pages and 42 more named assertions; neither candidate has a Chrome-only comparison gain, and no names are missing. No supplement repeats were indicated.

`python3 -B compare_followup.py RAW_DIRECTORY BATCH COHORT` consumes only an existing final report. The inherited check verifies the exact allowlist, disjointness from withheld, per-URL coverage, settings, and pin. Compressed directional sets are relative to the candidate, not the reference. Main results are recorded separately when finalized.
