# IndexedDB queued schema operations — Studio

Compared integrated `96fe56640258d78ddc0845e208ebfb77d2f3dd9b` with implementation `ae90e0b36690377935e5653bfdd7b1bdb3f209c7`. This is incremental to the deleted-store metadata result, not an additional claim for its 28 passes. No native bindings or clone callback changes.

The frozen `01f49f43` census and the fresh base point to two related clusters: eight assertions on operations issued before store deletion/recreation, and twelve on index population interleaved with writes and deletes. Native schema edits ran immediately, while ordinary requests ran later against mutable names, index lists, and replacement key generators. The implementation keeps synchronous handle metadata but orders backing schema changes and constraint checks in the transaction's request queue. Partitioning, IDL bindings, serialization, general transaction scheduling and small-value event timing are separate work. No withheld or native fault-injection cases were included.

| 64 matched URLs | Base | Candidate | Repeat | Signed Chrome | Gained | Lost |
|---|---:|---:|---:|---:|---:|---:|
| Named passes | 314 | 334 | 334 | 342 | 20 | 0 |
| Fully passing pages | 48 | 56 | 56 | 64 | 8 | 0 |

All gains match Chrome. The two deleteObjectStore assertions and three createIndex assertions each gained in window, dedicated, shared and service workers. `delta.json` lists every URL, name and occurrence and the exact page changes; regenerate with `python3 compare.py` here. A page requires complete status, OK harness, and nonempty all-pass subtests. The baseline dedicated-worker createIndex page had a harness error; all other harnesses were OK, with zero restarts or watchdog kills in all four runs. Eight unchanged Chrome-only names remain on small-value ordering (four) and cross-connection/overlapping-scope scheduling (four). No extrapolation to the full suite.

`controls/schema.html` is a separate local regression, not counted in the WPT totals. On baseline the replacement store continued the old generator (9 instead of 1); candidate and Chrome both passed the entire test, including checking record contents, the next generated key, unaffected sibling, schema rollback and success/error/abort ordering on reopen. Matched traces are in `controls/*.json`.

## Provenance and replay

- Pinned WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; Studio environment JSC `0e0dd7038a7a-6f4649e395e0`. Build command `CARGO_BUILD_JOBS=3 CARGO_TARGET_DIR=/tmp/r7-idb-eval/target pyro-build` after sourcing `~/Library/Caches/lightningpanda/pyro-env.sh`. Binary SHA-256: base `9a504cb1cd12dfdc87459fc025b8efae7054f31e9ccb16763cd73f7e727d9d19`; candidate and repeat `4d22ec72e27531b9e27accc0c06f49882545cbf250ff8fbd395253d6bf4d2b74`.
- Fresh signed Google Chrome `158.0.8127.37`, verified with `codesign --verify --deep --strict`, Team `EQHXZ8M8AV`. Same app and pinned SPKI as `../round7-studio-idb-metadata/README.md`; separate profile. `chrome` records the launcher.
- `urls.json` restricts pinned WPT to ordinary URLs with both frozen observations. `run.sh` records matched two-job invocations, fresh contexts and bind check, using the same `../round7-studio-idb-metadata/runner-ports.patch` (24820–24829). `repeat` points to the candidate binary. Raw receipts are included.
- Local control: set up a private static WPT root containing `controls/schema.html` and links `fonts` and `resources` to pinned WPT. From the runner use `bun runner/bench/wpt/run.mjs --wpt "$PWD/probe" --dirs controls --jobs 1 --bin "$label=$PWD/$label" --out "controls/$label.json"` for each binary/launcher. No edits to upstream WPT.
