# Typed shorthand and grid-line assignment

Independent QA tested production change `84213347eef308a7a844049eb5b4a0f03c15abc9` plus its native guard at `578977e675f32cbeeab72e49b53c7137ea9b455c`. The source branch starts from integration `6e0a8f847`. The behavioral comparator is the archived executable at `939d3d608`. No exact integration-parent executable was available. The comparison therefore includes intervening changes to editing and geometry.

All 3,142 assertions on the existing 43-page selection matched by URL, name and occurrence. Passing assertions increased from 3,033 to 3,045 (+12/−0), and fully passing pages from 23 to 25. All eight target margin length or percentage objects and all four target grid-line math objects now raise TypeError as expected. Every target passes in the archived Studio Chrome 158.0.8127.37 run. Individual comparisons are in targets.json. QA ran from 20:42:36 to 20:42:40 UTC on 2026-11-21 with the same pinned WPT, JSC and selector as the preceding batch.

Three native guards passed in 0.15 seconds. They include successful shorthand string and parsed-value writes, grid line strings and auto, a typed margin longhand and preservation of the original declarations after rejected assignments. The one-job candidate build took 47 seconds and the native build took 44.95 seconds.
