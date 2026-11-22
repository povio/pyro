# Whitespace after a soft line break

At a soft break, the browser carries the visible whitespace at the start of the new line into the edited DOM. Consecutive collapsible spaces in a split text node count as one. Alternating ordinary and nonbreaking spaces keep that width while allowing wrapping. Preserved whitespace remains literal. The ordinary paragraph separator has its own behavior.

Production source c8a480ca5cdef7c262b6227982fe59ae7fe9587f was tested against integrated 4d25d12111cb91c892034ca533bf07241e515386. The parent executable came from e8d39ab42 and has SHA-256 eaf0086f435ac2050586e4f7de4b8d40f77eab35a4e3d29214ec7daa9783137b. The entire intervening diff contained documentation and evidence. Candidate SHA-256 is 6a76bba4795364c4595252f96901479497be28ab6731aa543c5e22e0068aae13. Reference was the archived signed Chrome 158.0.8127.37.

Independent QA verified 12,713 → 12,721 passes across 13,060 matched static assertions (+8/−0). All eight gains pass Chrome. The targeted chrome-compat insertlinebreak page rose from 3/11 to 11/11. Its adjacent insertparagraph page retained 11/11. Fully passing pages rose from 39 to 40 of 54. The separate testdriver Enter cohort retained all 2,852 outcomes (2,675 pass, 11/27 pages fully passing). The original static parent and signed reference receipts were reused. Enter used the archived signed reference and a fresh runtime-equivalent parent run. The named ledgers include every URL, name, occurrence and result.

In 24 independent cases, exact post-break markup/text agreement rose from 12 to 16. Normal and nowrap each rose from 0/6 to 2/6. Pre and pre-wrap retained 6/6 each. Exact caret agreement remained 8/24 and event sequence agreement remained 12/24. Undo restored starting markup and anchor in all 24 candidate cases. Additional owner probes measured different suffixes and break positions, with markup agreement rising from 12 to 24 of 24 in the suffix matrix and 12 to 42 of 48 in the edge matrix.

## Native guard disposition

The original exact contenteditable_line_breaks_arrows_and_words guard passes at 4d25d1211 and fails on c8a480ca5 at the first markup check. It expects an ordinary space before “world” after Shift+Enter in hello[] world. Signed Chrome and the candidate both serialize that space as &nbsp;, with the caret at offset zero of the following text node. After two ArrowRight presses, ArrowUp, ArrowDown, End, and word Backspace, candidate markup, focus text and offset agree with Chrome at every measured step. Both end with hello followed by a BR and one NBSP. native.json.gz records all seven observations for each binary.

Commit d8ad7aca updates only the four whitespace literals in the native guard. Arrow navigation, offsets, input types, history and word deletion expectations remain exercised. The updated exact guard and paragraph_insertion_in_a_phrasing_host_uses_the_caret_style each ran one test and passed. Browser measurements retain the original c8a480ca5 binary attribution.
