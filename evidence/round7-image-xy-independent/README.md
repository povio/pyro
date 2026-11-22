# Independent image-coordinate QA

Exact parent 1a7ae7aa5 and candidate fe38f3cc1, clean arm64 wptiter/render builds with the same pinned JSC. Candidate binary and portable tar.zst are in the MacBook shared cache at ~/Library/Caches/lightningpanda/round7-qa-image-fe38f3cc1. The build provenance and archive digest identify them.

The frozen WPT selection at a421f2fea5eb completed on both binaries and signed Chrome 158.0.8127.37. Parent passed 0/6, candidate 5/6, Chrome 6/6. The remaining named failure is “x and y properties for image elements with css zoom 1”. The doubly zoomed image has y=109 instead of 112. Direct inspection of the fixture with its assertion scripts removed showed the parent and candidate already lay out the preceding one-pixel image at height 1 instead of Chrome’s 4. The transformed image also reports y=110 instead of 120. Both are visible in the zoom-direct receipts. Track zoomed intrinsic image sizing separately.

Seven direct snapshots recorded 112 scalar fields across scrolling, transforms, fixed positioning, relayout, zoom, hiding, and detachment. All 82 parent fields matching Chrome stayed matched. The candidate matches 94/112. Sixteen coordinate differences equal the window scroll offset. Two zoomed offsets already differed in the parent. The draft requires scaled, untransformed border-edge coordinates relative to the initial containing block. After nested scrolling the ordinary image is (110,430) on both engines. After window scroll (30,200) Pyro retains (110,430) and Chrome returns (80,230). Pyro reports the fixed image at (41,219), Chrome at (11,19). Hidden and detached images return (0,0).

Recommend development integration for the five signed gains, retaining the nested-zoom failure and separately documenting the spec/Chrome window-origin difference. The independently exercised entry point is the built executable. Native Rust tests were not run. The retained f2d6805e0 binary was excluded.

[CSSOM View image coordinates](https://drafts.csswg.org/cssom-view/#dom-htmlimageelement-x)
