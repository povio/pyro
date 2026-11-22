# Prior passing occurrences lost

These rows compare current LP with prior LP on the same URLs. The JSON includes the signed Chrome status, exact occurrence key, original shard ordinal, raw diagnostics and receipt digests for each observation. Both ordinal assignments ran on the local machine.
There are 147 lost canonical occurrences across 26 URLs. Current LP reported 135 named failures, 2 named timeouts, 2 reported Not Run and 8 absent names. Signed Chrome passed 54, failed 81, and had no exact match for 12.


| Diagnostic group | Occurrences | URLs | Observed behavior |
|---|---:|---:|---|
| formatblock-query-empty | 73 | 1 | The operation returned an empty formatBlock queryCommandValue where a block tag name was expected. |
| popup-name-not-cleared | 44 | 12 | COOP popup retained its name where the assertion expected it cleared. |
| generated-signature-name | 6 | 1 | The prior original name contains a generated signature string absent under the exact canonical key. Related same-page observations are listed separately; they do not replace its score. |
| targeted-form-referrer | 6 | 2 | Targeted form submissions with noreferrer included a nonempty referrer. The row records the element and target attribute. |
| null-body-status | 4 | 1 | Observed HTTP status was 200 rather than the expected no-body status. |
| linebreak-leading-space | 2 | 1 | The observed markup introduced a nonbreaking space after the inserted line break. |
| link-boundary-insertion | 2 | 1 | Text insertion at a link boundary left the link text unchanged. |
| moved-script-load-event | 2 | 2 | A moved prepared script fired an unexpected load event. |
| reported-not-run | 2 | 1 | The runner emitted the name with Not Run status; it did not produce an assertion result. |
| window-location-missing | 2 | 1 | Two previously passing sec-fetch-user operations were not emitted on a page with a current timeout. |
| canvas-read-exception | 1 | 1 | An image-drawing assertion expected a DOM exception on pixel readback; none was reported. |
| named-timeout | 1 | 1 | The current runner recorded a subtest timeout. |
| popup-resize-timeout | 1 | 1 | A popup resize-event test timed out before it reached an assertion. |
| worker-static-data-import | 1 | 1 | The dedicated-worker static import assertion returned false. The receipt does not identify the underlying fetch or module failure. |

Missing exact names, subtest timeouts and observed failed assertions retain distinct statuses. Groups describe the recorded symptoms. The underlying implementation cause is unverified.
