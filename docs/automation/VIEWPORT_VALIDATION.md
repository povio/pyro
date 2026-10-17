# Protocol viewport integration

`Emulation.setDeviceMetricsOverride` previously acknowledged requests without changing native
window metrics and accepted negative dimensions. The retained regressions reproduce both defects.
The command now validates first, applies the browser's persistent page viewport, and reports
unsupported mobile, transform and orientation options without mutation. Clear/zero disables
metrics overrides while preserving unrelated media/input preferences.

The implementation consumes exact browser viewport dependency
`39373a662e3c45eb377e5c81c794a56a838f8640` and MutationObserver dependency
`f800bc16daf7862001fb1d390db020359701afba`, above approved main `da1a6b3`.
Only CDP, tests and protocol documentation are authored in this increment; dependency code is
unchanged. Native viewport storage/event delivery and observer scheduling have separate owners.

On the affected macOS 26.7.1 M1 host, SDK MacOSX26.5, locked debug validation passes all 373
workspace tests, including 115 protocol tests. Strict workspace clippy, formatting and diff checks
pass. The release protocol suite also passes 115/115. Real Playwright 1.59.1 passes 27/27
checks in both debug and release. The new viewport check verifies width/height,
DPR, screen size, native resize delivery, current media-query matches, sibling-page isolation,
and dimensions before init scripts after navigation. The previously failing locator fill/click
check passes unchanged with native MutationObserver.

Native protocol regressions cover current/new worlds, navigation persistence, reset and zero
normalization, field types/ranges, and atomic refusal of unsupported options. Width/height/screen
values are bounded integers; DPR is finite/nonnegative, with zero normalized before the native API.
No polling or recurring task was added to the implementation; the browser schedules coalesced
resize delivery only when viewport state changes.

Explicit orientation is accepted only if already equivalent to the native screen orientation
(primary type derived from dimensions, angle zero). Playwright's desktop portrait request with
landscapePrimary remains unsupported until the native orientation binding exists. Mobile layout,
visual viewport transforms, MediaQueryList change events and complete CSS layout remain gaps.

The successful workspace run does not resolve the earlier raw-JSC bmalloc construction SIGTRAP
on candidate166277a. Its executable text/system dependencies matched unchanged main; failedbinary,
OS report and comparison artifacts remain preserved separately. No crash fix is claimed here.

TOS's last unchanged f9f run used candidate166277a before MutationObserver and failed0/2 after
UA setup progressed to routes, with missing MutationObserver and URLSearchParams. This local
fixture result is not a new TOS run or a CPU/memory comparison. Previous benchmark binaries and
TOS run artifacts remain pinned.
