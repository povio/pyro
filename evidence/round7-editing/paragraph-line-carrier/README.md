# White-space on a single formatting element moved between paragraphs

The joined even and odd occurrence ledger at c3372ad21 recorded 216 signed-only failures on 60 variants of join-different-white-space-style-paragraphs.html. Every historical URL/name/occurrence triple reproduced on integrated parent 381c8f952. Its executable came from 2f65e816c, with twelve intervening evidence-only paths. The adjacent build receipts identify source, tree, executable and pinned JSC. Candidate source is 12639aaecc77a9f2d988e2d2afb37044d0ca16af.

When the extracted line consists of one inline formatting element, that element can carry the paragraph's whitespace. The previous join always added an outer span. The candidate records the carrier's original mode before a partial line is extracted, retains existing inline whitespace declarations, and leaves any following line in the original paragraph.

Independent serial QA confirmed 3,456 matched target occurrences on pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The 216 Fail to Pass transitions all pass signed Chrome 158.0.8127.37. All 3,240 parent passes held. Fully passing pages rose from 0/60 to 60/60. The target signed observations were reused from the pre-block-joins archive. The named ledgers contain URL, subtest name, one-based occurrence, parent, candidate and signed status.

In the 24 direct controls, exact post-edit markup agreement improved from 0 to 8 and computed white-space from 20 to 24. Both selection endpoints and computed color matched Chrome in all 24. The split and unsplit controls cover both directions and declarations using var(), inherit and !important. Original formatting-element connectivity matched Chrome in 12 cases and original text connectivity in zero, on both Pyro builds. All undo return values matched Chrome, while full post-undo snapshots including identity differed in all 24 on both builds.

The preceding paragraph-to-line cohort retained all 1,455 passes. The static editing guards retained all 13,060 statuses and the eight child-line variants retained all 300 statuses, including 29 and 60 Pyro-only passes respectively. These guard parent and signed arms were reused from the preceding integrated topic. The candidate arms were measured here.

All 16 remaining literal markup differences were accounted for by whitespace and trailing semicolons in style attributes. Comparing these 24 fixtures after normalizing that punctuation gave 24/24 matches. Undo restored the starting markup in 24/24 candidate cases.

A separate serial comparison on the 86 other variants in the pre-block-joins selector covered reverse block/line joins, preformatted joins, plaintext deletion and input-event target ranges. All 5,780 statuses matched the current parent, including 219 Pyro-only passes. Both Pyro arms were measured here and the signed arm was reused from the archived 206-page comparison.
