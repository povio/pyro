# Window indexedDB accessor QA

Fresh builds of exact parent 00b7ab0bf and candidate bdbb41a00. Only the supplied descriptor test was added to the parent. All four pinned ordinary IDL globals were run serially on the parent, candidate, candidate repeat and signed Chrome 158.0.8127.37. All 828 named assertions matched across arms. The sole change was “Window interface: attribute indexedDB” from fail to pass. The candidate, repeat and Chrome passed all 828. All four harnesses completed on every arm. comparison.json includes the 30 historical worker gains, all passing on every arm. Their names came from the archived 1d0a43b93 worker comparison.

The supplied native descriptor test ran once on each arm. Only the parent’s Window.prototype placement assertion failed. Candidate and Chrome expose the accessor on the global only and agree on all 14 direct probe results. All seven factory and object prototype identity controls passed in each of four globals on both Pyro arms and Chrome. Native dedicated worker owner partition and committed catalog tests passed on both arms.

Separately, interfaces_are_lazy_globals_and_opaque_origins_reject failed at its initial Window IDBKeyRange getter expectation on both arms, so later assertions in that test were unexecuted. The native IndexedDB group passed 21/23 on the patched parent and 22/23 on the candidate.

provenance.txt lists parent, parent tree, candidate, candidate tree, WPT revision and reference signing identity. The matched JSC cache key was 0e0dd7038a7a-6f4649e395e0. The prior worker binary was not reused. Raw logs, JSON, scripts and binary hashes accompany this report.
