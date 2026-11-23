# Paragraph insertion at a void-element caret

The combined census at c3372ad21 recorded 36 signed-only failures on editing/other/insert-paragraph-in-void-element.tentative.html. Every URL/name/occurrence triple reproduced on 0a354f99c. The measured parent executable was built at 246c0a72c. The integration tip adds evidence and relocates five rustdoc lines without changing executable code. The immutable candidate was built at edc8ba24df20c5658a73cdcad83c52e8d1412ab8.

A collapsed DOM selection inside an image or input resolves to its parent boundary for the split. The existing element moves into the right paragraph. For a break, optional break or embed followed immediately by text, the split leaves that text with the element. At the end of a heading a terminal break continues in the default paragraph separator. The probe records the resulting caret and connectivity independently of serialized markup.

| Target WPT | Parent | Candidate | Signed Chrome 158.0.8127.37 |
|---|---:|---:|---:|
| Passing occurrences | 6/72 | 42/72 | 40/72 |

All 36 gains pass signed Chrome. The two original Pyro-only passes, for a lone BR in a list item with and without a layout flush, held. The remaining 30 occurrences fail both browsers. No membership changed. The archived Chrome arm is from guard-main-014 at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. All named ledgers include URL, name, one-based occurrence, parent, head and signed status.

Nine paragraph-command and constrained-container guard pages (7,209 occurrences), 54 static pages (13,060) and eight child-line pages (300) had identical parent/head outcomes. The text-insertion void guard is included once in the static cohort. Static and child-line parent/signed receipts were reused from the preceding integrated topic. The paragraph-command comparison used the archived signed arm and newly measured parent and candidate.

The 54 direct controls cover six void tags, three block containers and three positions. Exact post-edit markup agreement with Chrome improved from 8/54 to 40/54, and both endpoint paths and offsets from 25/54 to 40/54. Original element connectivity matched in 51/54 on both binaries. All 54 original void elements retained zero DOM children. The remaining literal DOM and endpoint differences are the nine HR cases, three lone EMBED cases and the lone BR/WBR list-item cases. Undo restored the starting markup in all 54 candidate cases. The two list-item BR tests exercise a WPT expectation which Chrome fails even though Pyro passes.

Independent QA confirmed all 36 named gains and both retained parent-only passes. No parent-matching direct field was lost. Undo return agreed in 52/54 cases. Restored DOM agreed in 54/54, restored endpoints in 37/54, and connectivity of the original void node after undo in 0/54 on both builds. The native paragraph/select boundary guard passed.
