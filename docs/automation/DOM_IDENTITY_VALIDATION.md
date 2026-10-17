# DOM automation, page identity and protocol checkpoints

The protocol increment adds native DOM handle adoption/geometry, page-scoped user-agent mapping,
and a browser checkpoint after synchronous inspector commands. It includes published browser
stylesheet/checkpoint dependency `339b714` and persistent identity dependency
`0f456ecf5b8492d845bf7dbe0d16f54993179759`; the latter contains approved native identity
`2c79ea0` and main `db179afb29e60c39ab5bca6a213b97d22186c824`. No browser, runtime or WebAPI
source was authored by the protocol worker in this increment.

## Focused results

On the affected M1 host (macOS 26.7.1, SDK MacOSX26.5), the locked protocol suite passes
113 tests. The debug real Playwright 1.59.1 harness passes 25 of 26 checks:

- Raw DOM ElementHandle ownerFrame/boundingBox, native Input dispatch and existing network,
  interception, cookies, storage and browser binding checks pass.
- A stylesheet inserted by Runtime evaluation starts loading while idle, fires load, and changes
  computed display. Before the protocol checkpoint, this same test timed out after five seconds.
  It uses no page timer or subsequent CDP command to trigger the load.
- The exact TOS setup packet `{userAgent:"",acceptLanguage:"en-US"}` succeeds. Two pages in the
  same context keep distinct navigator/document/fetch/XHR identity. Invalid HTTP header values
  leave the prior override intact; reset restores defaults. Before mapping, context.newPage
  failed with the unsupported Emulation.setUserAgentOverride error.
- The unchanged locator fill/click regression still fails during Playwright injected-script
  setup because MutationObserver is absent. This remains a WebAPI dependency gap; the suite
  exits 1 and is not reported as fully passing.

Native protocol tests additionally cover cross-world backend-node adoption, object-group
release, stale node IDs after navigation/GC, bounded node descriptions/IDs, invalid/inert or
detached geometry, Runtime-only session handle cleanup, field validation and unsupported UA
client hints. Geometry remains the native synthetic layout, not full CSS box layout.

## Native workspace failure retained

The first combined debug workspace run aborted in the two-test `lp-jsc-sys` binary with
SIGTRAP (signal 5), after the browser, protocol, DOM and JS suites had passed. The triggered
stack starts with `pas_bitfit_directory_get_first_free_view`, then bmalloc allocation,
`WTF::AtomStringImpl::addLiteral`, `JSC::CommonIdentifiers`, `JSC::VM::VM`, and the raw
`evaluates_script_in_system_jsc` test. This is distinct from the prior finalizer SIGSEGV and
CF run-loop SIGTRAP signatures; no root cause or repair is claimed here.

The interrupted run completed 245 tests successfully before the raw sys failure. Running the
remaining net/runtime/WebAPI suites separately passed 105 more tests. Strict workspace clippy
and formatting checks pass. This is not a successful uninterrupted workspace run.

The same raw test binary subsequently passed five default-concurrency and five serial
process runs, all direct exits 0. Those repeats do not erase the original failure. W1 owns
runtime diagnosis. The original OS report and sanitized stack remain in ignored local
handoff storage; no runtime code or test assertion was changed to suppress the failure.

## TOS and performance limits

The preceding unchanged TOS f9fde4aa0d smoke on W2's baseline host failed before page
assertions: first at absent indexedDB.databases (a7dfe98), then at unsupported user-agent
mapping (86ff4ee). This increment fixes the latter packet in focused tests. A new exact
candidate run is required before claiming TOS progress beyond setup. Existing TOS artifacts
and benchmark binaries remain pinned; no CPU, memory or full TOS compatibility result is
inferred from these local HTTP tests.
