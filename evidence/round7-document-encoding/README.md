# JSON navigation encoding and iframe coverage

A JSON navigation on LP used the HTML parser and inherited the parent or HTTP charset. This corrupted UTF-8 text and interpreted markup and character references inside JSON strings. The candidate recognizes application/json, text/json and valid +json MIME essences. It presents their contents as literal text, decodes the response as UTF-8, strips a leading UTF-8 BOM, and reports the normalized response MIME as document.contentType.

RFC 8259 §8.1 requires interoperable JSON to use UTF-8. Section 11 defines no charset parameter and says adding one has no effect on compliant recipients. The HTML text-document loader uses a pre element and the PLAINTEXT tokenizer state. The pinned WPT applies UTF-8 even to an explicit conflicting HTTP charset. Signed Chrome stable 158.0.8127.37 still honors that parameter. This is an intentional compatibility difference: an incorrectly labeled legacy JSON response may display differently in LP and Chrome. [RFC 8259 §8.1](https://www.rfc-editor.org/rfc/rfc8259#section-8.1) · [RFC 8259 §11](https://www.rfc-editor.org/rfc/rfc8259#section-11) · [HTML text documents](https://html.spec.whatwg.org/multipage/document-lifecycle.html#navigate-text)

## Matched observations

The 14-URL selector is 6844c531f0fc03e30f94c59a271e79e556efb1d61a224576f0094948b752b9fa. Both LP runs reported 1,321 named occurrences. The parent passed 1,183 and the candidate passed 1,187, versus signed Chrome's 1,360 passes among 1,361 occurrences. The only gains were four previously observed JSON failures: three also pass in Chrome, one is the explicit legacy charset assertion that Chrome fails. No previously passing or previously reported name was lost. Strict pages improved from ten to eleven, versus thirteen signed. LP completed twelve pages and retained one harness error and one timeout in both runs.

| Frozen cohort | Parent passed / reported | Candidate passed / reported | Signed passed / reported |
|---|---:|---:|---:|
| Six encoding targets | 1142/1276 | 1142/1276 | 1316/1316 |
| Eight neighboring guards, including the JSON gap | 41/45 | 45/45 | 44/45 |
| JSON navigation page alone | 1/5 | 5/5 | 4/5 |

Both Document-characterSet-normalization pages already passed all 654 names on the measured parent. In the older RC.5 census they had timed out after 54 observed passes. Those 600 newly observed passes predate this patch. On the measured parent and candidate, the TextDecoder variant passed 168/168. The XMLHttpRequest variant reported 64 passes and 104 failures with "Too many pending XHR requests." The single-byte iframe page reported 128 passes and left 40 names unobserved on timeout. The unsupported-labels page reported 128 passes and 30 named timeouts. The 128-child-frame limit and 64-pending-XHR limit were unchanged. The six sequential and six concurrent ordinary iframe controls completed and agreed on labels and actual decoded text, as did the XHR/TextDecoder body for original bytes [128,233]. Existing native-fault holds remained excluded.

## Direct document observations

The response bytes for the JSON fixture encode the literal text `{"label":"雪 € <b> &amp;"}` as UTF-8. The following rows distinguish inherited and explicit charset selection. Entries are actual document.characterSet, document.contentType and body.textContent.

| Case | LP parent | LP candidate | Signed Chrome |
|---|---|---|---|
| No response charset, windows-1251 parent | windows-1251 · text/html · `{"label":"й›Є в‚¬  &"}` | UTF-8 · application/json · `{"label":"雪 € <b> &amp;"}` | UTF-8 · application/json · `{"label":"雪 € <b> &amp;"}` |
| Explicit windows-1251 response charset, UTF-8 parent | windows-1251 · text/html · `{"label":"й›Є в‚¬  &"}` | UTF-8 · application/json · `{"label":"雪 € <b> &amp;"}` | windows-1251 · application/json · `{"label":"й›Є в‚¬ <b> &amp;"}` |
| text/plain with windows-1251 charset and bytes DF F0 | windows-1251 · text/plain · Яр | windows-1251 · text/plain · Яр | windows-1251 · text/plain · Яр |

Direct receipts also cover UTF-8 and legacy parents separately, text/json, application/problem+json, uppercase MIME, explicit UTF-8 and UTF-8 BOM. The native split-byte test feeds the BOM and a multibyte character one byte at a time, with conflicting charset hints, and checks the literal source including angle brackets and entities.

## Reproduction and provenance

The coordinator's earlier integration point was 3c11af606. The actual shared parent binary was built by the editing owner at f21696ba724a922f477d52bf4eedf57d99e93f57 (tree f72f5c29627647b5ab50a6fa0ab5c3589c858d3d); production editing code differs from 3c11af606. The parent binary SHA-256 is ac4f5e15f81be22d847e3b9feea05cd313e751fbf793edb48878dcb5d7fe9aa9. Candidate 8385ae51e produced binary SHA-256 896b04621cc4a4dbcb45910972a6fc5d793773296bd5c86a088e87d50b115925. The final source adds only a string-reference correction in the native test assertion; no production inputs differ. Provenance records both revisions and trees, signed Chrome stable identity, original shard mapping and static patched JSC archive. WPT is pinned at a421f2fea5eb7f91e0db399e44b0fb0779ee1603.

Run `python3 run.py base` and `python3 run.py candidate` with the named binaries, then `python3 report.py`. The runner uses four jobs, wptserve, testdriver, HTTP/2, generated globals and no native retry. The direct commands were `node probes.cjs <label> <binary>` and `node json-document.cjs <label> <binary>`. Native commands are in native.sh. Every per-page and named result remains in the raw JSON, pages.json and named.jsonl.gz.

All 9 native tests passed on the final source. Independent QA evidence 67f4d3197e78bfebe4483979f3070149ad5bc44c confirms the 14-URL comparison and direct document observations.
