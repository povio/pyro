# CSS-generated KeyframeEffect lists

Transition source `cc1eab817` exposes two native computed endpoints in offset order, retains their values after the effect loses its target and returns independent frame objects. The existing CSS transition sampler still owns playback. The native guard exercised a variable endpoint, non-linear transition timing, copy construction, mutation of the returned objects, target removal and script replacement of the frames.

Independent QA compared the exact parent `e725340e1` with the candidate on pinned WPT a421f2fea. The two CSS Transition pages gained five assertions with zero losses and now pass 7/7. All five gains pass in the Studio Chrome 158.0.8127.37 reference. CSS Animations stayed at 1/32; the separate scripted Web Animations control stayed at 0/1 (signed local Chrome passes). Across all four URLs, the named score moved from 3 to 8 of 40. Both runs had zero restarts. The native guard passed in 0.08 seconds following a 35.21-second test build. The one-job candidate executable build took 31 seconds.

## CSS Animation follow-up

Exact candidate `bebfd14cb6b7e6d4b2cdab9a496226abcd307637` gained 26 assertions with zero losses against the transition checkpoint `cc1eab817`, and 31 with zero losses against exact parent `e725340e1`. The CSS Animation page reached 27/32; both transition pages held at 7/7. The separate WAAPI serialization control stayed at 0/1. All four harnesses completed. The candidate scored 34/40 overall.

Both native guards passed in 0.09 seconds after a 46.24-second native build. The one-job executable build took 115 seconds. Coverage includes offset order, relative units, live custom properties, a script-created effect preserving specified units, copy construction, transition target removal and script replacement of transition keyframes.

The implementation reads element-target animation declarations on demand; pseudo-element CSS Animation lookup is outside this batch. Four remaining animation assertions pass Studio Chrome (partial endpoints, custom-property declarations in two cases and timing-only endpoint insertion). A fifth residual involving filter serialization also fails Studio Chrome. The ordinary WAAPI control passes signed local Chrome. The named receipt records each reference and assertion.

## Endpoint completion and scripted serialization

Source `a85d50171d58ab68a11782217af73224ba3ea498` starts from integration `68561a1a2`. Independent QA measured the same four URLs against the archived `bebfd14cb` executable. The integration parent was unmeasured. Three named results improved and none regressed. All three gains pass signed Chrome 158. CSS Animations improved from 27 to 29 of 32 by completing a partially authored endpoint and retaining timing-only endpoint frames. CSS Transitions retained 7/7. Script-created WAAPI keyframe value serialization improved from 0 to 1/1. All four harnesses completed with zero restarts.

Two remaining CSS Animation cases expect custom properties to be absent from the returned frame. Both pass in Studio Chrome. Pyro includes their declarations when resolving keyframes. CSS Variables defines custom properties as discretely animatable and CSS Animations instructs the cascade to exclude properties that are not animatable. Both differences remain counted. The pinned test file matches the current upstream file at b418bf5d71d99f38f06426b60038da346556ad33 byte-for-byte. A third remaining failure concerns filter serialization and also fails in Studio Chrome. [CSS Variables](https://drafts.csswg.org/css-variables-1/#defining-variables) · [CSS Animations](https://drafts.csswg.org/css-animations-2/#keyframes) · [Upstream test](https://github.com/web-platform-tests/wpt/blob/b418bf5d71d99f38f06426b60038da346556ad33/css/css-animations/KeyframeEffect-getKeyframes.tentative.html)

The single-job executable build took 96 seconds. Both native guards passed in 0.08 seconds after a 68-second native build. Named results and exact binary provenance accompany this section.
