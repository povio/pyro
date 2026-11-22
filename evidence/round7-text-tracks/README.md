# WebVTT cue parsing and text tracks

The exact 59c4ba28f parent exposed an HTMLTrackElement and an empty TextTrackList, but no VTTCue or VTTRegion. Eighty-eight target URLs cover the ordinary WebVTT API, cue-text tokenizer, file parser and selected HTML track operations. Six guards exercise nearby DOM/encoding/media behaviors. No caption rendering, permission-policy or cross-origin fixture was added to the scope.

The parent's targets reported 5 passes out of 260 observed names, versus signed Chrome's 204 out of 266. Only 34 target pages completed: three had harness errors and 51 timed out. No target strictly passed. All six neighboring guards completed (LP 24/29, signed 28/29). All 94 selected URLs completed in the original signed run. Chrome stable 158.0.8127.37 does not expose VTTRegion; pass sets for region support are retained independently of signed-only gains.

The shared parent binary (249180360d7d21c0a5f1c92c0e9512c9705bbb052ffb79a9a87facbd1ddc8ad1) was built once and rehashed. The static pinned JSC, WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, complete manifest, signed shard provenance, selectors, binary digests and exact source trees are recorded alongside the raw receipts. Use acquire.sh for the candidate, run.py for each matched WPT run, and native.sh for focused native checks.

The captions control verifies a programmatic sorted cue list, timestamp bounds, fresh detached markup with speaker/language annotations and decoded text, identity during file replacement, real network requests and DOM events. The separate pending replacement control waits for the old same-origin request to start before switching src, then releases the older response after the replacement has loaded. The region control captures a timestamp processing instruction, ruby span and named character reference and tests real region properties where exposed.

## Matched outcomes

| Group | URLs | Parent named | Candidate named | Signed named | Strict pages parent / candidate / signed |
|---|---:|---:|---:|---:|---|
| target | 88 | 5 | 264 | 204 | 0 / 86 / 58 |
| guards | 6 | 24 | 24 | 28 | 5 / 5 / 5 |
| fileParser | 43 | 0 | 53 | 41 | 0 / 43 / 31 |
| cueParser | 6 | 0 | 79 | 79 | 0 / 6 / 6 |
| webvttAPI | 25 | 3 | 111 | 65 | 0 / 25 / 9 |
| mediaTrackObjects | 14 | 2 | 21 | 19 | 0 / 12 / 12 |
| unique | 94 | 29 | 288 | 232 | 5 / 91 / 63 |

Of 259 gained passes, 197 also pass in signed Chrome (191 previously observed nonpasses and six formerly absent names); 62 newly passing names are LP-only. No earlier pass disappeared. On the full 94-URL sample LP now has seven signed-only and 63 LP-only names (one LP-only name predates this work). Forty-eight LP-only occurrences explicitly name regions or occur on region-named pages, including implemented VTTRegion attributes and WebVTT region file parsing. Refer to named.jsonl.gz for the complete run-by-run classification. These bounded results do not establish aggregate non-CSS parity.

The candidate completes 93 pages and times out on one, versus 40 complete, 51 timeouts and three errors on the parent. It reports every named occurrence, with zero native restarts or watchdog kills. Ninety-one pages strictly pass. The remaining signed-only cases are five existing CSS ::cue selector assertions, an empty cue DocumentFragment child-count difference, and the pre-existing timeout in track-cue-mutable-fragment.html. The empty cue parser follows the current WebVTT empty node-list and isomorphic DOM construction rules. The pinned empty-cue test and Chrome expect one empty Text node. The timed-out fixture awaits both a text-track load and the counting video's canplaythrough; its fallback MP4 is MPEG-4 Part 2. That gating explanation is source/codec analysis, not an event trace from the timed-out WPT. The independent programmatic fragment-mutation control completed normally.

Cross-realm controls accepted true foreign cue/track objects in both directions and rejected a forged cue with TypeError. LP separately verified cross-realm region identity and rejected a forged region. A one-second H.264 clip with two cues active from 0.20 to 0.55 seconds produced A enter → B enter → A change → B change, then A exit → B exit → A change → B change, and paused before ended. Signed Chrome paused at approximately 0.554 seconds; candidate playback.json retains its observed position. During completed detach/reattach, both browsers retained the same track, same cue and load state with one GET. Equivalent URL spellings retained the same resources and emitted no extra load in both attached and detached cases. Replacing a genuinely pending URL dispatched error for the old request then load for the new, leaving only the new cue after the old response was released. The initial alias-staleness observation and its correction remain in iteration-d32 and iteration-365.

Rendering captions onto video, CSS pseudo-element styling and out-of-scope policy fixtures remained outside this comparison. The loader uses the existing browser network scheduler and origin policy. Direct control JSON records server requests and lifecycle events; full WPT receipts retain the page and named outcomes. [WebVTT cue text parsing](https://w3c.github.io/webvtt/#cue-text-parsing-rules) · [HTML time marches on](https://html.spec.whatwg.org/multipage/media.html#time-marches-on)

Final native checks passed 4 Rust tests and 10 JavaScript fixture tests covering existing media, track fetches, transcript descriptors, cue/token/region identity, Web IDL descriptors, two-track chronology and pauseOnExit.

Independent QA evidence f9652584ac1d4366f934d52422c632b6002958e6 confirms the 94-URL named comparison and existing ordinary controls on immutable executable bed69812b37d59726a9acac2526364068d12685cb7541a262476564fca916e17.
