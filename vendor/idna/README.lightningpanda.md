# idna 1.1.0, patched

The published `idna` 1.1.0 crate (crates.io; part of [servo/rust-url](https://github.com/servo/rust-url)),
used through `[patch.crates-io]` in the workspace `Cargo.toml`. License: MIT OR Apache-2.0, as upstream.
`.cargo_vcs_info.json`, `Cargo.toml.orig`, `Cargo.lock`, `benches/` and `tests/` were left out of the copy,
and the matching test, bench and dev-dependency sections were removed from `Cargo.toml`.

## The change

`src/uts46.rs`, `Uts46::to_ascii_from_cow`: when validation fails for a host that uses the URL deny list and
is entirely ASCII with no forbidden domain code point, the lower-cased host is returned instead of an error.
`new URL('https://xn--a/')` and `https://xn--ab-j1t/` parse in browsers, and WPT `url/IdnaTestV2`,
`url/toascii` and `url/url-origin` require it. A host with a non-ASCII character still fails on an invalid
label (`xn--a.\u00df`). Only the error path changes, so valid hosts cost the same.
