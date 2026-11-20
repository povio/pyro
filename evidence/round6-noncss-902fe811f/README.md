# LP non-CSS census at 902fe811f

This snapshot reports 710 of 30,513 selected URLs. The full pinned inventory has 31,827 variants. All 1,314 withheld URLs retain a coverage reason. Pending observations also remain explicit in inventory.jsonl.gz. The historical-loss ledger was sampled at 838 URLs and covers all 292 canonical historical lost occurrences.

The LP binary SHA-256 is 9f2bfe099b2d87b0626040695a0583d460c9f7b3c8a401941761c2d9b6595cb4. Its CI JSC is 0e0dd7038a7a-6f4649e395e0. WPT is a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Each browser runner uses four jobs and the published WPT profile. This census used up to three sharded runners concurrently. Individual timing outcomes can vary.

Local Chrome stable 157.0.8103.80 and prior LP 4c13e694a are the original same-revision macOS arm64 results from the [published census](https://github.com/povio/pyro/tree/57a9450abdaa792dafae09f6e84d4236fa72a2ea/evidence/round6-noncss-census). The original census records exact controller versions and public references. This snapshot reuses those local receipts. It does not rerun Chrome.

pages.jsonl.gz stores URL, environment and three triples (page status, harness status, fully passing). named.jsonl.gz stores URL, canonical name, one-based occurrence and three pairs (reported status, original name). The browser order is current LP, prior LP, local Chrome. Null means missing. Canonicalization applies only to encoded WPT ports authenticated against the actual server configuration.

The run command and exact eligible selections are derivable from start.py, inventory.jsonl.gz and the pinned runner. To reproduce the reports with the original manifest-runs cache, run python3 compare.py, python3 inventory.py and python3 historical-losses/update.py. The checksums cover all distributed artifacts.
