# Live font-face descriptors

`CSSFontFaceRule.style` now exposes a same-object declaration block backed by Stylo's font-face descriptor parser rather than CSS property parsing. Set, remove, enumeration, aliases and cssText use the same typed descriptors and write through to the owning stylesheet. The focused Rust integration test exercises identity, invalid assignment, preserving a sibling descriptor, removal and forwarding cssText.

Matched WPT on Studio, same WPT/JSC/server and Chrome stable as `system-fonts-and-transition-properties.md`. The focus includes css-fonts, css-font-loading and urange-parsing (199 URLs). Parent: 151 strict / 8,080 passing names. Candidate: 157 / 8,442. Stable: 168 / 8,624. Gains/losses: 6/0 pages, 362/0 names. Remaining limitations include `auto` in the font style/weight/width range descriptors and tokenizer handling of comments inside Unicode ranges.

| 8,082 main URLs | Strict | Passing names | Observed names |
|---|---:|---:|---:|
| Parent `8e9326a9` | 5,405 | 174,581 | 194,312 |
| Candidate | 5,416 | 174,968 | 194,312 |
| Chrome stable 158.0.8127.37 | 6,950 | 182,963 | 194,912 |

Full candidate/parent exclusive: 11/0 pages, 387/0 names, 0/0 observed names. Candidate/Chrome exclusive: 245/1,779 pages, 5,539/13,534 names, 319/919 observed. LP: 7,864/101/105 completions with harness status 0/1/2, 11 timeouts, one hang. Chrome: 7,986/23/62, ten timeouts, one hang. Supplemental 41 (last LP 27/436; Chrome 35/619) and excluded 39 remain separate, not rerun. H2 and testdriver coverage limitations are unchanged.
