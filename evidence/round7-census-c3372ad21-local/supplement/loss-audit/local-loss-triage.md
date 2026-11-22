# Local prior-pass loss triage

All 74 occurrences across 32 URLs were reconstructed from the immutable final named ledger, independently checked against the published loss stream, then joined to current, previous LP and signed Chrome raw receipts. The JSON retains the full original names, occurrence, exact statuses, diagnostics, three receipt checksums and per-engine page/harness states. No affected page was replayed.

Current LP reported 66 named failures on complete/OK pages, two named timeouts and six missing exact names. One timeout occurred on a timeout/Timeout page; the other occurred on an error/Error cloned ImageBitmap page. Both previous and current LP reported the same page-level image-decode rejection there, although this particular name previously passed. The named-frame timeout previously completed and passed. There is no positive transport-failure diagnostic for either case. All six missing names occurred on complete/OK pages. There were no runner-stall placeholders among these 74. Signed Chrome passed 52, failed 12 and had no matching name for 10.

| Diagnostic group | Occurrences | URLs | Recorded behavior |
|---|---:|---:|---|
| popup-name-not-cleared | 36 | 11 | COOP iframe-opened popup retained its name where the assertion expected it cleared. |
| dangling-target-name-stash | 5 | 1 | Target-name round-trip stash result differed for base/form/link target cases. The failure message does not expose the returned value. |
| formatblock-merged-siblings | 4 | 1 | formatBlock produced one div with a br instead of two sibling divs. |
| analyser-diagnostic-name | 3 | 1 | A still-passing constructor error case includes different TypeError message text in its generated subtest name. |
| form-target-referrer | 3 | 1 | Button-targeted form submissions with noreferrer sent a nonempty referrer. |
| embedded-image-width | 2 | 2 | The query-encoding embed probe reported fallback width 300 instead of image width 2. This alone does not establish a URL encoding failure. |
| flat-tree-text-search | 2 | 1 | Text searches inside shadow-root dialogs returned false, including a slotted-text case. |
| formatblock-value | 2 | 1 | formatBlock returned an empty query value instead of div. |
| null-body-status | 2 | 1 | Worker fetch observed 200 for the expected 204 POST/304 OPTIONS response. |
| protocol-port-name | 2 | 1 | A literal %s followed by a dynamic wptserve port appears in this generated name; the unchanged canonicalizer does not rewrite this token. |
| video-image-decode | 2 | 1 | createImageBitmap rejected two cross-origin/redirect video sources with an InvalidStateError decode diagnostic. |
| cloned-image-timeout | 1 | 1 | The cloned ImageBitmap subtest timed out on a page reporting a harness error. |
| early-hints-title-name | 1 | 1 | The old filename-derived passing name was absent. Current LP and signed Chrome both reported a descriptive failing assertion on the same complete page. |
| inert-frame-focus | 1 | 1 | Sequential focus remained on the preceding element rather than the expected iframe. |
| named-iframe-timeout | 1 | 1 | The noopener/named-iframe case timed out before a final opener assertion. |
| navigation-name-stash | 1 | 1 | Cross-origin navigation with noopener returned an unexpected stash value. The raw assertion does not identify the wrong field. |
| opaque-window-interface | 1 | 1 | A data-URL iframe exposed FileSystemDirectoryHandle when the assertion expected it absent. |
| plaintext-designmode-bold | 1 | 1 | Bold markup was absent from the designMode/plaintext-only result. |
| sw-text-import-loaded | 1 | 1 | A service-worker text import reported LOADED when the assertion expected FAILED. |
| text-after-link | 1 | 1 | Insertion at the end of a link placed the new character outside the link. |
| transparent-pixel | 1 | 1 | A drawImage pixel was transparent black instead of opaque red. |
| worker-static-data-import | 1 | 1 | The dedicated-worker static data-URL import assertion returned false; no lower-level diagnostic was supplied. |

The largest exact diagnostic spans 36 popup name-clearing failures on 11 URLs. The six stash mismatches and three form referrer assertions have distinct reported symptoms. The two named timeouts lack a completed assertion. None of these observations assigns causality to an individual commit.

Three missing audio constructor names were regenerated with changed TypeError text. The corresponding calls pass under their new names. Two missing protocol-handler names include the raw run-specific port after an unescaped %s token; the equivalent calls pass separately in all three receipts. One old early-hints name matched the filename; current LP and signed Chrome both reported a descriptive failing assertion. These related observations do not change the original occurrence keys or scores.
