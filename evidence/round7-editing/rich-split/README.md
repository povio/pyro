# Inline formatting at rich paragraph edges

Integration parent 0e6341d8b4404ce168c4ad3544960ec04b2fb436 and measured parent source 4b6249f2f4c269a33c823e9fcfe4d1e3f147a9f3 differ only under docs/validation. The MacBook runtime receipt at 2055f24ee identifies the shared binary, which was rehashed before running the eight parent shards. Its SHA-256 is 1d142e841e98b7d762b65b2e935a28c393c9df04f3b1367b15cc64f110f4cb6a. The standalone RC.5 census at 516dc3159 supplied the historical ranking. Its evidence at 5892e136b is a separate measurement. All 26 corrected URL, name and occurrence triples appear in that signed-only ledger and reproduced on the integration parent.

At the edge of inline formatting in an authored rich P, DIV or LI, insertParagraph keeps the placeholder BR in the formatting chain on the empty side. The insertion caret on a trailing split belongs inside the copied formatting, so subsequent typing retains it. If a terminal padding BR follows the caret, it stays on the preceding line and the new line gets its own formatted break. A following editable text run continues to receive the split normally.

The first candidate 56add2283 gained all 26 signed cases but lost two parent-only passes on a bare span directly under an editing host. Its generated anonymous paragraph was being treated as an authored block. Corrected source 49c28a188dcbde1858d15a0aadd7166876d6df64 recovers both prior passes and retains every signed gain. Each revision's complete ledger is included.

| Eight insertParagraph shards | Parent | Corrected candidate | Signed Chrome 158.0.8127.37 |
|---|---:|---:|---:|
| Named passes on WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 | 7,005/7,195 | 7,031/7,195 | 7,021/7,195 |

All 83 Pyro-only passes held. No names appeared or disappeared. There are 73 remaining signed-only failures on the shards. The 54 static editing pages, eight child-line pages and void-paragraph page retained all 13,060, 300 and 72 statuses respectively. Parent and first candidate guard arms were measured in this batch. The corrected head reused those arms.

In 72 direct cases, exact serialized markup after the split and after typing improved from 30 to 57; both endpoints improved from 45 to 63. These probes cover nested bold/italic, inline color, links, a following editable or locked run, and a trailing break. Remaining markup differences concern locked tails and a trailing BR following a link. An additional 36 neutral-span, nested-link and inline-block cases improved from 8 to 30 exact markup matches and from 20 to 36 endpoint matches. No measured parent-matching field was lost in either cohort.

In all 108 cases, two undo calls restored the starting markup and both endpoints. On both Pyro builds, one undo removes typing and a second reverses the split, while signed Chrome groups the edits. Original node connectivity after undo differs in all cases on both builds.

Independent QA reran all eight WPT shards and the 72+36 corrected head controls. It confirmed the named counts and all direct observations. The native paragraph/select subtree guard passed.
