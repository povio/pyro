# Reading-flow and reading-order declarations

Source `f58a64328eb8e6a24abe2fbe5d083c379edb341b` was tested against its exact parent `7d932ecfac472594bbe6ba537338a558e8886882`. Stylo parses the seven CSS Display 4 reading-flow keywords and reading-order integers, including calc expressions, and exposes cascaded and computed values through CSSOM. Both properties are non-inherited by default. The focus traversal and layout algorithms were outside this change. [CSS Display 4](https://drafts.csswg.org/css-display-4/#reading-flow)

Independent QA measured two separate profiles on pinned WPT a421f2fea, JSC 0e0dd7038a7a-6f4649e395e0. The six CSS declaration pages rose from 18/40 to 40/40 (+22/−0), closing four previously failing pages and preserving all 18 invalid-value controls. All 40 outcomes match archived signed Studio Chrome 158.0.8127.37. Six direct declaration/cascade observations also matched live signed Chrome. The native guard covers initial values, explicit inheritance, var substitution, integer rounding and invalid replacement. It passed in 0.07 seconds after an 84-second native build. The single-job executable build took 211 seconds.

Separately, four testdriver sequential-focus pages retained 2/12. The normal control passed 2/2 on both binaries; source-order remained 0/5, flex-flow 0/4 and grid-columns 0/1. Signed Chrome passed all 12. All ten harnesses completed without browser restarts.

## Representative cost

Eight serial fresh browser processes ran in ABBA/BAAB order. Each fixture had one warm-up and five measured samples per process. Samples applied and checked declarations on 64 elements eight times. The ordinary fixture wrote and checked order; the candidate-only fixture wrote and checked both new declarations. Wall medians include the CDP evaluation, and the reported CPU is mean process-tree CPU per sample.

| Binary | Fixture | Median wall ms | Mean CPU ms |
| --- | --- | ---: | ---: |
| reading-base-7d932ecfa | ordinary | 3.42 | 2.5 |
| reading-f58a64328 | ordinary | 3.41 | 3.0 |
| reading-f58a64328 | reading | 4.73 | 4.0 |
