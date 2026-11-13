# url 2.5.8, patched

The published `url` 2.5.8 crate (crates.io; [servo/rust-url](https://github.com/servo/rust-url)), the latest
release, used through `[patch.crates-io]` in the workspace `Cargo.toml`. License: MIT OR Apache-2.0, as
upstream. `.cargo_vcs_info.json`, `Cargo.toml.orig`, `Cargo.lock` and `tests/` were left out of the copy, and
the matching test and dev-dependency sections were removed from `Cargo.toml`.

Every URL in the engine goes through this crate: the `URL` binding, `<a>`/`<area>`, `Location`, navigation,
fetch and the network stack. The changes bring it to the current WHATWG URL Standard where 2.5.8 follows an
older one. Each changed place carries a `pyro:` comment.

## The changes

`src/parser.rs`
- `parse_file` (file, file slash and file host states) is rewritten to the standard: a host next to a Windows
  drive letter is kept (`file://host/C:/`), the base's host carries over to relative and drive-letter inputs
  (`C|` against `file://host/dir/file` is `file://host/C:`), and `localhost` still becomes the empty host.
- `parse_path`: empty leading segments of file paths are kept (`file:////foo`, `file://spider///`); the
  path is no longer trimmed. A `..` segment shortens the path by the standard's "shorten a URL's path", which
  only keeps a lone drive letter for `file:` (`..` against `abc://x/y/z/C:/` pops `C:`). The per-character
  check that inserted a slash after a drive letter and clearing the host on a drive letter are gone.
- `parse_path_start` always writes the leading slash of a special URL, also after an empty file host.
- `parse_relative`: a special URL skips every slash and backslash before the authority (`///test` against
  `http://example.org/` is `http://test/`; 2.5.8 failed). A non-special URL needs exactly `//`, and a
  backslash is an ordinary path character there (`\a` against `foo://foo/a` is `foo://foo/\a`).
- The path percent-encode set includes `^`.
- Opaque paths: a space right before `?` or `#` is written as `%20` (`non-special:opaque  ?hi`).

`src/lib.rs`
- `set_query(None)` and `set_fragment(None)` no longer strip trailing spaces from an opaque path (the
  standard dropped that step; the parser change above keeps the serialization round-trippable).
- `set_path`: a URL without a host gets `/.` before a path that starts with an empty segment
  (`non-spec:/.//p`) and loses it when the path no longer does.
- `set_host_internal`: setting a host on such a URL drops the `/.`.

`src/quirks.rs`
- `set_port`: only the empty string clears the port; a value that is empty after removing tabs and newlines,
  or starts with a non-digit, leaves the URL unchanged.
- `set_pathname`: an empty pathname leaves a non-special URL with an empty host (`foo://`) with an empty path.

`src/origin.rs`
- The origin of a `blob:` URL is the inner URL's origin only for `http:` and `https:`.

## Cost

The changes are on the file, opaque-path, relative-slash and setter paths. For http(s) the path loop does
less work than before (the per-character drive-letter check is gone); a release-mode loop over typical
absolute and relative http(s) URLs measured the same ~290 ns per parse before and after.

## Tests

`crates/pyro-webapi/tests/url_conformance.rs` runs WPT's `urltestdata.json`, `setters_tests.json` and
`toascii.json` (a committed subset, plus the full upstream files when a WPT checkout is present) through the
calls the bindings make and through the JavaScript bindings. Two of upstream's own unit tests no longer
hold and were not copied: `test_strip_trailing_spaces_from_opaque_path` (the stripping is gone) and the
`FileWithHostAndWindowsDrive` syntax violation (the standard no longer reports it).
