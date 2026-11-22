# insertHTML in a designMode editing host

Tested source 8e75e8d967f0afafc72aba2dbe8c762477c8385a against exact integrated parent 9345b73f262a57cbec0bd41619f299afdb4c633f. The parent executable SHA-256 was 865dc29f2ef3ee08b569b723bf27598ae25c616c2a70f6153c06d0a9835d7916. The candidate executable SHA-256 was 608ea8f138109afcedd9ac2f87a6a0ad6e123484ed5bdb1ec2cc38a0b9e0f207. The two private build receipts record Git tree, pinned JSC, compiler and flags.

The HTML insertion algorithm chooses rich markup or plain-text conversion from the receiving editing host. In designMode, the document body can own a selection inside a plaintext-marked descendant. Selecting B in ABC and inserting bold lowercase b then yields A&lt;b&gt;b&lt;/b&gt;C. A separate plaintext host behind a noneditable boundary or inside a shadow root still receives plain text. Rich-formatting command eligibility continues to use the selection's context.

## Eight historical census losses

The frozen local even-shard comparison was prior Pyro 1f1be032436bfeee9f48cdb5acbc0f71d21d5af9 against c3372ad21df84bb4bb7386ae4f114fa185d10f30, with signed Chrome 158.0.8127.37 and WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. historical-eight.json is an unmodified row extract, including the original reported names, occurrence, statuses, diagnostic and shard. eight-loss-disposition.json joins each original row to the fresh exact-parent, candidate and signed results. The local scope excludes Studio's share and the previously held cases. [Authoritative census audit](https://github.com/povio/pyro/blob/16e4765bcfd9013f0c19f28c4044de8f2bf42f4f/evidence/round7-census-c3372ad21-local/supplement/loss-audit/local-loss-triage.json)

| Historical group | Occurrences | Prior → frozen Pyro | Signed Chrome | Integrated parent → candidate |
|---|---:|---|---|---|
| plaintext-only-in-designMode, formatted HTML insertion | 1 | Pass → Fail | Pass | Fail → Pass |
| formatblock: selected DIV and following bare text, query after command | 2 | Pass → Fail | Fail | Fail → Fail |
| formatblock: heading/paragraph siblings converted to DIV, compare innerHTML | 4 | Pass → Fail | Fail | Fail → Fail |
| inserttext: caret at the end of a link, compare innerHTML | 1 | Pass → Fail | Fail | Fail → Fail |

For the six formatBlock assertions, the current implementation and Chrome return an empty query value across separate resulting DIVs or serialize the converted lines as one DIV separated by a BR. The pinned fixtures expect the first DIV's tag or two DIVs, respectively. The link assertion expects the typed character inside the closing A; Pyro and Chrome place it immediately outside. All seven candidate failure messages matched the new signed run verbatim. These are the previously documented compatibility choices. This patch changes only the remaining signed-passing insertHTML case.

## Focused WPT and direct controls

The 38-page single-worker wptserve/testdriver comparison had 8,564 identical names in each arm. Pyro passed 8,057 → 8,058 (+1/−0); all 269 parent passes that failed signed Chrome were retained. Fifteen pages fully passed on both Pyro arms. All harnesses completed with status 0 and zero browser restarts. The target page rose from 1/3 to 2/3. Its bold assertion fails in both Pyro and Chrome; its Enter assertion passes in both Pyro builds and fails on Chrome. The current focused profile is distinct from the census shard profile.

In the owner's 36 stable-mode cases, post-edit markup agreement rose from 32/36 to 36/36. The four changes covered a plaintext descendant of the designMode body, a plaintext child of a rich host with designMode on and off, and a rich child of a plaintext host with designMode off. The last case correctly strips markup according to the outer plaintext host. Plaintext inside a locked island, a locked descendant and shadow hosts retained the parent behavior. Both anchor and focus agreement rose from 26 to 30/36. Return values and enabled state matched Chrome in all 36; event sequence agreement stayed at 21/36. Pyro still emits an additional beforeinput in scripted insertHTML cases.

A separate 36-case matrix captured the entire live body during designMode changes. Undo restored the original live body in 36/36 on both Pyro builds and signed Chrome. Pyro's existing HTML-based history can reconstruct descendants, so a reference to a detached original element can still show its edited contents after undo. In six off→on cases where the mode was enabled after setting the selection, Pyro's parent and candidate both reset the range to the document start; Chrome retained the range in the target. The resulting body mismatch already occurs in the exact parent. Both native filters passed, including a direct designMode/locked-host/undo guard.

Independent QA repeated the candidate on all 38 URLs and both direct matrices. Its final receipt used the exact candidate executable, had zero restarts, and agreed on every page status, name and subtest status. All 72 direct rows agreed with the owner measurements. The independent run used the archived parent and signed receipts as comparators.
