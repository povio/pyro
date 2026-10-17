# Build-provenance audit of earlier shared-target benchmark binaries

**Question:** earlier Worker 3 comparisons built their binaries into one shared Cargo target directory
(`/private/tmp/lp-w3-network-target`). That procedure once silently reused stale artifacts (see
`../lazy-interfaces-08ad9e2/`). Did the earlier benchmark binaries actually contain their stated sources?

**Method:** each SHA was rebuilt in a fresh, empty target directory, from the original worktree path where it
still existed, with the same toolchain and settings: rustc 1.99.1, LLVM 23.1.1, `SDKROOT=…/MacOSX26.5.sdk` and
`cargo build --locked --release -p lightningpanda`. The `__TEXT,__text`, `__TEXT,__const`, `__TEXT,__cstring` and
`__DATA_CONST,__const` sections were then compared using `bench/scripts/macho-section-hash.py`.

- **Whole-file hashes cannot be used.** Two fresh builds of `e43a776` from the same path, in two different target
  directories, have different whole-file SHA-256 (`7bf095ec…` and `cb314d9f…`).
- **The four sections are reproducible.** Both fresh `e43a776` builds have identical hashes for all four sections.

| Historical binary (whole-file SHA-256) | Used in | Fresh isolated rebuild | Code/constant sections |
| --- | --- | --- | --- |
| `f537c86` (`73323263…`) | PR #30 parent | from `/private/tmp/lp-automation-tos-batch` | identical |
| `e43a776` (`8b1a0c95…`) | PR #30 candidate | from `/private/tmp/lp-w3-cpu-candidate-e43a776`, twice | identical |
| `aca9b34` (`cd943748…`) | causal baseline for W1's unpromoted experiments | from `/private/tmp/lp-w3-cpu-baseline-aca9b34` | identical |
| `41356eb` (`2bbe638d…`) | PR #24 rewrite side | from a new detached worktree (original path no longer at that SHA) | identical |

**Conclusion:** each audited historical benchmark binary contains the machine code and constants of its stated
source. The published PR #24 and PR #30 numbers stand; no correction is needed.

One detail looked suspicious. The historical `e43a776` build log does not list `Compiling lp-js`, although
`e43a776` changes only `lp-js`. Its sections nevertheless match fresh `e43a776` builds and differ from `f537c86`.
The `lp-js` artifact for that source path was therefore already current in the shared target, most likely from
the focused `class_lengths` test run there. The log alone did not prove this; the section comparison does.

**Limits:**
- Only these four sections are compared. Other sections and debug info are not. Different code in the same
  sections would show up; identical sections do not prove identical linking of everything else.
- The upstream Zig binary of PR #24 (`cf1545da…`) was built separately with Zig, outside the shared Cargo target,
  and is not part of this audit.
- Fresh rebuilds are on the measurement host at `/private/tmp/lp-w3-audit/`, with their build logs.

**Procedure from now on:** build every compared SHA in its own fresh target directory. Alternatively, verify the
binaries with `macho-section-hash.py` against fresh builds before timing.

`sections.json` lists the per-section hashes of every binary audited here, including the lazy-interface pair
and its two discarded shared-target binaries.
