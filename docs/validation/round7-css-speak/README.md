# CSS Speech speak property

Tested source `42fa67fa672c45c71b78eb6e0c5110a30ad9e118` introduces the inherited CSS property with initial value auto. Its specified and computed keywords are auto, never and always. The native guard verifies typed and string writes, inheritance, rejection of invalid replacements and inheritance after deletion. Scope is declaration parsing, cascade, CSSOM and Typed OM. Audible speech output was outside this implementation and validation. [CSS Speech specification](https://drafts.csswg.org/css-speech-1/#speaking-props-speak)

The source branch starts at `a5a7386a7`. Independent QA compared the candidate against the archived `578977e67` executable, using the preceding 43-page selector and pinned WPT, JSC and Studio Chrome 158.0.8127.37. The source base also contains intervening popover/DOM edits. [Selector and runner provenance](../round7-stylepropertymap/README.md)

Passed assertions rose from 3,045 to 3,053 of 3,142 (+8/−0). Fully passing pages rose from 25 to 26. All eight gains were on speak.html, which reached 34/34. The other 42 pages were unchanged by name. The signed Studio Chrome run passed 31/34 on speak.html. Five gains matched Chrome (initial, inherit, unset, revert and a custom-property reference). The three other gains cover the specified keywords auto, never and always. There were no missing or added names, restarts or watchdog kills.

The initial one-job build failed a generated Stylo copyability assertion after 16 seconds. The corrected candidate built in 201 seconds. The native guard passed in 0.07 seconds after an 85-second test build. The change introduced no speech synthesis or output-device dependency.
