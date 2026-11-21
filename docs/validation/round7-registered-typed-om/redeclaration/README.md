# Stylesheet order after removal and reinsertion

On the prior executable, the length-to-integer registration change returned the correct unit when the test ran alone. Running it after the preceding test removed its two stylesheets returned the stale length unit. Both diagnostic readbacks are preserved here. Document stylesheet insertion compared reusable node IDs. The fix selects the next stylesheet by its owner element's position in the live document.

Production change `777412906` is based on `7a2b3b00c`. Tested source `a92a105d0b2cf508ecb83d994fb307c1c328ee2f` adds the native guard. Independent QA reused the measured `6886fc523` comparator, its seven URLs and the archived Studio Chrome 158 reference. Named passes rose from 421 to 422 of 423 (+1/−0), closing the last of the original 31 registered-property targets. The separate computed-map code-point ordering assertion still fails. All seven harnesses completed with zero restarts.

Two native tests passed in 0.09 seconds after a 70-second native build. They cover registration replacement and removal, plus typed list, specified, universal and unregistered controls. The one-job executable build took 126 seconds. The insertion change applies to document stylesheets. Its insertion cost has not been measured in this run.
