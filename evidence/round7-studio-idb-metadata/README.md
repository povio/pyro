# IndexedDB deleted-store index metadata — Studio

Base `c6600a40c066b59e903940ba7a2257a98bf2ca3c`, implementation `2256e6a0704039e8e4ec4ac6ca3de9bff6252030`. Only `idb.js` changed; no change to the clone callback, storage backend, request queue or event dispatch.

## Selection

In the frozen non-CSS census `01f49f43`, partitioning coverage (31 Chrome-only named occurrences on one page) depends on cross-site partitioning, and IDL harness (31 across variants) aggregates unrelated bindings. Storage cloning and Blob/File key paths (18 and 16) need separate serialization/brand work. The largest verified, tightly scoped cause was stale index names on deleted store handles: 12 index-metadata rollback, 12 mixed-metadata rollback and 4 rename/abort failures. Fresh failures on the integrated base agreed with the WPT assertions. Deleted stores and stores newly created by an aborted upgrade now clear their index metadata; the existing snapshot still restores original index identities on rollback.

## Matched results

34 measured ordinary URLs, fresh contexts; exact URL/name/occurrence changes and residuals in `delta.json` (`python3 compare.py`). Only URLs with both frozen observations and no withholding reason were selected. No native fault or withheld variants were exercised.

| | Base | Candidate | Repeat | Signed Chrome 158 | Gained | Lost |
|---|---:|---:|---:|---:|---:|---:|
| Named passes | 80 | 108 | 108 | 116 | 28 | 0 |
| Fully passing pages | 18 | 30 | 30 | 34 | 12 | 0 |

All gains match Chrome, in window, dedicated, shared and service workers. All 34 pages completed with harness status OK, zero restarts and zero watchdog kills on every recorded run. Generator/data rollback, existing store and index restoration, explicit commit and abort request event ordering are included. Eight remaining Chrome-only passes on four deleteObjectStore pages concern requests queued before schema deletion and same-name recreation, not deleted-handle metadata. No whole-area extrapolation.

## Provenance

- WPT checkout `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; pinned CI JSC `0e0dd7038a7a-6f4649e395e0`, static library SHA-256 `cdd57b33b09f8b2e7d7648d63dc88fbd1f1d444ecd08eea91fe67db192773c0b`.
- Studio `pyro-build`, `--locked --profile wptiter --bin pyro`, three Cargo jobs, private target. Base binary SHA-256 `3a827a4c8d3305ebc845a822dc0a872cc7e5b1502b9716ea0e327f485ec23afc`; candidate `75322d56e6524c5c61cbb6ccd1a003ab2bffc548c8851266b7a3ca0a6827619f`.
- Signed Google Chrome stable `158.0.8127.37`, Team `EQHXZ8M8AV`. Private `ditto --noextattr --norsrc` copy from mounted image passed `codesign --verify --deep --strict`. Executable hash `b87bcebb575e53b0f29a4eeeb6480ec9c9bc3537690cacd52d6c67c2c3e3908e`, framework `a04d264c815c317ba1ffa89d388144ec639ec0e4a1a114a2ba524ad4bdd130c8` match frozen reference. Chrome launcher trusts the pinned WPT certificate SPKI for service worker registration.
- `run.sh` and `chrome` record the local commands. Runner from base with `runner-ports.patch` only, Playwright Core 1.59.1; discovered exactly the frozen allowlist with worker globals, two WPT jobs, private wptserve ports 24820–24829. This is a matched local profile, not a replay of the original four-job HTTP/2 census. The exact-head repeated candidate and fresh signed reference are included separately.
