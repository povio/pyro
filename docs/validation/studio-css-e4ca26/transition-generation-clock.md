# Transition generation time

`436b0a12` uses the transition generation timestamp for pending CSS transition playback. The ready promise is still fulfilled asynchronously. Specifying a start time in script remains a separate path.

Six jobs, same 15 URLs, ten runs per binary. `generation-time-repeats.json.gz` contains all per-run results, including failures.

| Named assertion | flexsuggest | flexcrossbound | genstamp |
|---|---:|---:|---:|
| Onscreen and offscreen transitions have the same start time | 5/10 | 6/10 | 10/10 |
| Start time is based on when transitions are generated | 10/10 | 9/10 | 10/10 |
| The start time can be set to seek a transition | 0/10 | 0/10 | 0/10 |

Matched main (8,082 URLs): gained one strict page and one name versus flexcrossbound, no lost pages or names. LP: 5,556 pages and 178,661 names; Chrome stable 158.0.8127.37: 6,950 and 182,963. The separate 41-URL supplemental cohort was unchanged at 27 pages and 577 names for LP, versus 35 and 619 for Chrome. The 39 withheld URLs remain unscored. LP 0.0.17; JSC `0e0dd7038a7a-6f4649e395e0`; WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. `difference-genstamp-*` records gains and losses in both directions; `outcomes-genstamp-*` retains nonpassing and missing outcomes.
