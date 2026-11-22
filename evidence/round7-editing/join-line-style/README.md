# White-space when joining the first inline line into a paragraph

The frozen even and odd census at c3372ad21 recorded 468 Chrome-only occurrences across 48 variants of join-different-white-space-style-left-paragraph-and-right-line.html. All 468 URL, name and occurrence triples independently reproduced against integrated parent e0f4f468b. The twelve changed paths from measured source 4478f9e89 to the integrated parent contain evidence only. The parent and candidate receipts identify the two executable digests and pinned JSC.

The candidate is 2f65e816c4f547b344c2dd606484f7b1081f5c26. The join compares the moved carrier's source white-space with the destination paragraph's inherited mode. A stylesheet-derived difference becomes inline formatting on the moved carrier. An inline declaration, including inherit or a variable reference, continues to resolve in its new context. A partial carrier's following line retains its original formatting.

| WPT cohort | URLs | Parent passes | Candidate passes | Signed Chrome passes |
|---|---:|---:|---:|---:|
| Target white-space joins | 60 | 987/1455 | 1455/1455 | 1455/1455 |
| Static editing guards | 54 | 12823/13060 | 12823/13060 | 12988/13060 |
| Child-line deletion | 8 | 300/300 | 300/300 | 240/300 |

All 468 gains were matched Fail to Pass transitions. Membership and every guard status were stable, including 29 static and 60 child-line Pyro-only passes. The named ledgers encode URL, name, one-based occurrence, parent status, candidate status and signed status. All WPT arms used a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Target baseline and candidate runs used a serial wptserve/testdriver runner. Signed 158.0.8127.37 target observations came from the archived pre-block-joins run. Guard parent and reference arms came from child-line-delete/revision2.

In 16 owner script-deletion controls with complete and split carriers, the computed white-space on the bold child matched Chrome in 14 cases, up from 12. Both inherit cases improved. The two equal-mode class cases still differed. All 16 restored their original markup on undo. Exact serialized markup matched Chrome in zero cases on either Pyro build. Pyro usually retains the span and Chrome transfers formatting to its child. The selected string, anchor node name and offset agreed in all 16 on both builds. Original carrier connectivity matched Chrome in eight cases and original text connectivity differed in all 16 on both builds. The probes and named WPT assertions measure distinct contracts.

Independent QA remeasured the frozen candidate. Fully passing variants rose from 12/60 to 60/60, matching the signed reference. All 1,455 occurrences matched membership across the three arms. Independent head probes confirmed the two remaining computed-style differences and found no lost parent-matching field. Undo succeeded in all 16. The complete post-undo snapshot, including original node connectivity, differed from Chrome in all 16 on both Pyro arms.
