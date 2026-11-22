# Selection endpoints across shadow roots

Tested 3051207311c379953aa88502d99cc854c9d2ddc6 against exact parent 368fcf95e007f2a1666d2506c790c73abb3f9dfb. The frozen candidate SHA-256 is cde91dcb5a1af8eaeb7269dc310b0f533b43e739dff8e3eaf33a88f8ae512ed9. The frozen parent SHA-256 is 82172fcac40ebeed0a2028fc4ad404b4cf6d247012cde0ddd545ea6bd391f309. Both private builds used the same pinned JSC. The build receipts record the trees, compiler, profile and JSC archive digest.

For setBaseAndExtent across different roots, the legacy anchor, focus and live Range collapse at the later endpoint in shadow-including DOM order. The composed range keeps both endpoints and the selection retains its direction. Extending into another root collapses the selection and composed range at the new focus.

## WPT

Independent QA confirmed six unique gained assertions, all signed Chrome 158 passes. The 54-page static cohort gained five with zero losses across 13,060 names, increasing from 12,780 to 12,785 passes and from 40 to 43 fully passing pages. Range-isPointInRange gained one, Selection-getComposedRanges-slot gained two and Selection-isCollapsed gained two. Independent QA separately verified Selection-direction improved from 6/7 to 7/7.

The 18-page selection/testdriver cohort includes those same five static gains and the direction gain. It rose from 4,770 to 4,776 of 4,786 with zero losses, and 12 to 16 fully passing pages. All pages completed with zero restarts. The static Chrome receipt was reused and the selection/testdriver comparison used a fresh run through the signed Chrome 158 launcher. The named ledgers retain every occurrence, including the existing range-update and deleteFromDocument failures.

## Direct controls and native guards

In 16 independent open/closed-root scenarios, legacy anchor/focus, type, direction, isCollapsed, live Range endpoints, point membership, and allowed/rescoped StaticRange snapshots all agreed with signed Chrome on the candidate. The 48-case owner matrix included reversed endpoints, distinct shadow roots and slotted text. All 48 candidate endpoint, direction, collapsed-state and composed-range observations matched.

Selection.toString() remains different in four independent cross-root setBaseAndExtent cases. The parent and candidate both return an empty string where signed Chrome returns selected text. The live Range string matched Chrome in all 16 cases. The direct archives keep the strings and endpoints separately. Both exact native filters ran one test and passed.
