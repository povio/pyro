# Offset dimensions and empty atomic inline boxes

Same pinned binaries, revision, six-job runner and URL inventories as `replaced-inline-containment.md`, except for the two new LP builds. On `css/css-sizing/aspect-ratio/replaced-element-043.html`, passing names rose from 115/180 to 163/180 to 180/180 (Chrome stable: 180/180). Two different CSSOM issues were involved: a 7.5px border box sometimes reported 7 depending on its position, and an empty inline canvas reported the 4px occupied by its margins. In stock Studio Chrome 158.0.8127.37, an absolutely positioned canvas and div of height 7.5px each reported offsetHeight 8 at top 0, .25, .5 and .75px. The fractional geometry regression test passed. Client and scroll dimensions were not changed; a separate 20.6px width probe at a fractional offset still reports clientWidth and scrollWidth 20 in LP versus 21 in Chrome.

| LP build | Main strict pages / passing names | Strict +/− | Names +/− | Observed names | 37 controls: strict / names |
|---|---:|---:|---:|---:|---:|
| `4af12369` | 5,499 / 177,843 | — | — | 194,540 | 13 / 852 |
| `48786ecc` | 5,499 / 177,891 | +1/−1 | +49/−1 | 194,540 | 13 / 900 |
| `11e189b2` | 5,500 / 177,908 | +2/−1 | +18/−1 | 194,534 | 14 / 917 |

Main (8,082 URLs): Chrome passed 6,950 pages / 182,963 names; Chrome-only 1,706 pages / 10,750 names, LP-only 256 / 5,695. The final run recorded 13 timeouts and one hang, Chrome ten timeouts and one hang. Supplemental (41 URLs): unchanged sets, LP 27 / 577 vs Chrome 35 / 619, both observed 874; 39 withheld URLs remain uncounted.

The first main run lost the offscreen transition ready-time page. In six matched six-job repeats it passed 3/6 on the parent and 4/6 on each candidate. The second main run recovered that assertion but timed out on transform-origin-in-shadow (one formerly passing name) and transform-hit-testing (five formerly failing names). Three matched reruns per binary completed on both URLs each time: one passing assertion and zero passing assertions respectively. The full-run losses are preserved, not replaced by repeat results. `offset-geometry-repeats.json.gz` contains the 24 control runs; `difference-*` and `outcomes-*` retain exact URL/name sets and missing observations.
