`Inter-subset.ttf`: a subset of Inter 4.001 Regular (the static "Inter 24pt" instance Google Fonts distributes),
Copyright 2016 The Inter Project Authors (https://github.com/rsms/inter). Licensed under the SIL Open Font License,
Version 1.1 (https://openfontlicense.org); Inter declares no Reserved Font Name.

It keeps the characters of `E2E-DUN-MONITOR-OPEN 1,234.56 (EUR) x-y AB xqx` and U+0301 with every layout feature, so
`tnum` (tabular figures), `calt`/`case` (hyphens and parentheses raised between capitals) and `ccmp` still apply.
Made with HarfBuzz 15's `hb-subset --unicodes=<those code points> --layout-features='*' --layout-scripts='*'
--name-IDs='*' --name-languages='*' --glyph-names --notdef-outline`.
