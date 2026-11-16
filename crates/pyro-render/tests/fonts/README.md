`Inter-subset.ttf`: a subset of Inter 4.001 Regular (the static "Inter 24pt" instance Google Fonts distributes),
Copyright 2016 The Inter Project Authors (https://github.com/rsms/inter). Licensed under the SIL Open Font License,
Version 1.1 (https://openfontlicense.org); Inter declares no Reserved Font Name.

It keeps the characters of `E2E-DUN-MONITOR-OPEN 1,234.56 (EUR) x-y AB xqx` and U+0301 with every layout feature, so
`tnum` (tabular figures), `calt`/`case` (hyphens and parentheses raised between capitals) and `ccmp` still apply.
Made with HarfBuzz 15's `hb-subset --unicodes=<those code points> --layout-features='*' --layout-scripts='*'
--name-IDs='*' --name-languages='*' --glyph-names --notdef-outline`.

`Inter-variable-subset.ttf`: a subset of Inter 4.001's variable font (`Inter[opsz,wght].ttf` as Google Fonts
distributes it, https://github.com/google/fonts/tree/main/ofl/inter), under the same copyright and license. It keeps
the same characters, without U+0301, and the font's variation tables: the axes `opsz` (14 to 32, default 14) and `wght`
(100 to 900, default 400), and an `avar` table that bends the weights above 400. Made with HarfBuzz 15's
`hb-subset --unicodes=<those code points>`.

`Ahem.ttf`: the Ahem test font, version 1.50, as web-platform-tests keeps it (`fonts/Ahem.ttf`). Its name table
says: "The Ahem font belongs to the public domain. In jurisdictions that do not recognize public domain ownership of
these files, the following Creative Commons Zero declaration applies" (http://dev.w3.org/CSS/fonts/ahem/COPYING).
Its ascent and descent (0.8 and 0.2 em) differ from Inter's, so it stands in for a fallback font of another height.

`COLR-palettes-test-font.ttf`: the COLR test font of web-platform-tests (`css/css-fonts/resources/`), Ahem's boxes
drawn in colour, with 16 CPAL palettes of 8 entries (palette 2 is marked light, palette 3 dark). Its name table says,
as Ahem's: "The Ahem font belongs to the public domain. In jurisdictions that do not recognize public domain
ownership of these files, the following Creative Commons Zero declaration applies"
(http://dev.w3.org/CSS/fonts/ahem/COPYING).

`NotoSansJP-kana_test-subset.otf`: web-platform-tests' subset of Noto Sans JP (`css/css-fonts/resources/`), the kana
`す` and `ペ` with their `palt` metrics and a kerning pair between them. Its name table says "© 2014-2020 Adobe
(http://www.adobe.com/)"; Noto Sans JP is licensed under the SIL Open Font License, Version 1.1
(https://openfontlicense.org).
