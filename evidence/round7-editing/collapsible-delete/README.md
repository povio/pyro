# Deleting the last characters after collapsible spaces

The joined census at c3372ad21 recorded 64 Chrome-only occurrences across the backward and forward deletion tests for characters following collapsible white-space. All 64 URL, name and occurrence triples reproduced on exact integrated parent 480f13b1a26436a4f7af8688a47486e63a61a39e. A single parent build was required because its WaveShaper and editing sources differed from earlier binaries. The candidate is 86eb29506746bde53eb2ff00394e92d9aa115e7d. The adjacent receipts retain executable and JSC hashes, source trees and build settings.

When deleting a suffix exposes an ordinary space at the end of a line, the edit preserves the width the space had before deletion. A visible run of collapsed spaces becomes one NBSP. If the deleted text was the only visible content, a BR occupies the empty line in its original position. Neighboring paragraphs remain in place. Character deletion, selection deletion and word deletion share this operation. Preformatted text retains its whitespace.

| Target WPT | Parent | Candidate | Signed Chrome 158.0.8127.37 |
|---|---:|---:|---:|
| Backward deletion | 8/41 | 40/41 | 38/41 |
| Forward deletion | 8/41 | 40/41 | 38/41 |

The 64 gains were matched Fail to Pass transitions, all passing Chrome. Four existing Pyro-only passes remained. Two occurrences continued to fail both browsers. Named ledgers contain URL, name, occurrence, parent, head and signed status. Both Pyro arms ran on the two pinned WPT URLs at a421f2fea5eb7f91e0db399e44b0fb0779ee1603 using wptserve and testdriver. The signed WPT arm was reused from the frozen reference census. Separate serial parent and candidate runs preserved all 13,060 static guard statuses and all 300 child-line statuses.

The 32 real-key probes combine normal/pre-wrap with ordinary, doubled, leading and nonbreaking spaces. They exercise Backspace, Delete, Alt+Backspace and Alt+Delete. Exact post-edit markup agreement rose from 22/32 to 32/32; caret path/offset from 24 to 32 and original text-node connectivity from 28 to 32. All edit event sequences and undo return values matched Chrome. Undo restored markup and caret in 32/32 on both builds. Original text connectivity after undo diverged from Chrome on both in all 32 cases.
