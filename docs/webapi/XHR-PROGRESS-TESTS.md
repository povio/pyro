# XHR progress delivery test contract

The pointer-repair WebAPI release run retained one failure in `xhr_upload_events_fire_before_headers_for_a_request_body`: two identical final **download** progress records appeared where its complete event string expected one. Later isolated passes were not sufficient to resolve that failure.

The native delivery pump reports newly received bytes and, when a later pump observes terminal completion without new bytes, reports final progress before DONE/load/loadend. The [XHR response-end-of-body algorithm](https://xhr.spec.whatwg.org/#handle-response-end-of-body) requires a final progress notification; body processing can also report progress. Equal loaded counts therefore do not by themselves imply a duplicated request or upload event.

A controlled real-HTTP fixture sends all six response bytes in a chunk, then holds the terminal chunk behind a channel gate. With the owner queue held until transport completion, the delivery coalesces into one progress event. If the owner consumes the data before the gate opens, completion produces a second progress event with the same byte count. The new test asserts both complete event sequences, the response body, upload-before-header ordering and the intermediate LOADING state. It introduces no product delay or retry.

The original upload-order assertion string is unchanged. Its setup now explicitly selects the coalesced schedule by waiting for the public loader activity to become idle before pumping owner tasks. The activity guard is released after the transport sink completes. A bounded test-only wait fails if that boundary is not reached; it does not make progress by rerunning the request. Production XHR/network behavior is unchanged.

This closes a delivery-scheduling assumption in that test once the controlled and complete WebAPI validation pass. It does not imply complete XHR conformance, all native tests green, or a new full742 TOS result. Original failure evidence remains in `evidence/pointer-focus-7973176/pointer-focus-webapi-release.log`.
