# Prior passing occurrences lost

These rows compare current LP with prior LP on the same URLs. The JSON includes the signed Chrome status, exact occurrence key, original shard ordinal, raw diagnostics and receipt digests for each observation. Both ordinal assignments ran on the local machine.
There are 221 lost canonical occurrences across 58 URLs. Current LP reported 201 named failures, 4 named timeouts, 2 reported Not Run and 14 absent names. Signed Chrome passed 106, failed 93, and had no exact match for 22.


| Diagnostic group | Occurrences | URLs | Observed behavior |
|---|---:|---:|---|
| popup-name-not-cleared | 80 | 23 | COOP iframe-opened popup retained its name where the assertion expected it cleared. |
| formatblock-query-empty | 73 | 1 | The operation returned an empty formatBlock queryCommandValue where a block tag name was expected. |
| generated-signature-name | 6 | 1 | The prior original name contains a generated signature string absent under the exact canonical key. Related same-page observations are listed separately; they do not replace its score. |
| null-body-status | 6 | 2 | Worker fetch observed 200 for the expected 204 POST/304 OPTIONS response. |
| targeted-form-referrer | 6 | 2 | Targeted form submissions with noreferrer included a nonempty referrer. The row records the element and target attribute. |
| dangling-target-name-stash | 5 | 1 | Target-name round-trip stash result differed for base/form/link target cases. The failure message does not expose the returned value. |
| formatblock-merged-siblings | 4 | 1 | formatBlock produced one div with a br instead of two sibling divs. |
| analyser-diagnostic-name | 3 | 1 | A still-passing constructor error case includes different TypeError message text in its generated subtest name. |
| form-target-referrer | 3 | 1 | Button-targeted form submissions with noreferrer sent a nonempty referrer. |
| embedded-image-width | 2 | 2 | The query-encoding embed probe reported fallback width 300 instead of image width 2. This alone does not establish a URL encoding failure. |
| flat-tree-text-search | 2 | 1 | Text searches inside shadow-root dialogs returned false, including a slotted-text case. |
| formatblock-value | 2 | 1 | formatBlock returned an empty query value instead of div. |
| linebreak-leading-space | 2 | 1 | The observed markup introduced a nonbreaking space after the inserted line break. |
| link-boundary-insertion | 2 | 1 | Text insertion at a link boundary left the link text unchanged. |
| moved-script-load-event | 2 | 2 | A moved prepared script fired an unexpected load event. |
| protocol-port-name | 2 | 1 | A literal %s followed by a dynamic wptserve port appears in this generated name; the unchanged canonicalizer does not rewrite this token. |
| reported-not-run | 2 | 1 | The runner emitted the name with Not Run status; it did not produce an assertion result. |
| video-image-decode | 2 | 1 | createImageBitmap rejected two cross-origin/redirect video sources with an InvalidStateError decode diagnostic. |
| window-location-missing | 2 | 1 | Two previously passing sec-fetch-user operations were not emitted on a page with a current timeout. |
| worker-static-data-import | 2 | 2 | The dedicated-worker static import assertion returned false. The receipt does not identify the underlying fetch or module failure. |
| canvas-read-exception | 1 | 1 | An image-drawing assertion expected a DOM exception on pixel readback; none was reported. |
| cloned-image-timeout | 1 | 1 | The cloned ImageBitmap subtest timed out on a page reporting a harness error. |
| early-hints-title-name | 1 | 1 | The old filename-derived passing name was absent. Current LP and signed Chrome both reported a descriptive failing assertion on the same complete page. |
| inert-frame-focus | 1 | 1 | Sequential focus remained on the preceding element rather than the expected iframe. |
| named-iframe-timeout | 1 | 1 | The noopener/named-iframe case timed out before a final opener assertion. |
| named-timeout | 1 | 1 | The current runner recorded a subtest timeout. |
| navigation-name-stash | 1 | 1 | Cross-origin navigation with noopener returned an unexpected stash value. The raw assertion does not identify the wrong field. |
| opaque-window-interface | 1 | 1 | A data-URL iframe exposed FileSystemDirectoryHandle when the assertion expected it absent. |
| plaintext-designmode-bold | 1 | 1 | Bold markup was absent from the designMode/plaintext-only result. |
| popup-resize-timeout | 1 | 1 | A popup resize-event test timed out before it reached an assertion. |
| sw-text-import-loaded | 1 | 1 | A service-worker text import reported LOADED when the assertion expected FAILED. |
| text-after-link | 1 | 1 | Insertion at the end of a link placed the new character outside the link. |
| transparent-pixel | 1 | 1 | A drawImage pixel was transparent black instead of opaque red. |

Missing exact names, subtest timeouts and observed failed assertions retain distinct statuses. Groups describe the recorded symptoms. The underlying implementation cause is unverified.
