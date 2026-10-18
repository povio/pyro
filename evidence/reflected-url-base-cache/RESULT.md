# Bounded URL reflection experiment

Main `4e89ab401854ea67da8df1ae01ca477b101b6c91` versus production candidate `84ab914ff17afc2f0dc3a21e126f9b50dfc36782` (tree `b89b0d460ff990e621e991096f8e9ca9d9b36f71`). Both were built in the same clone with locked release dependencies, DEBUG=0, root strip=none, verified allow-jit and explicit polling traps. Immutable binary hashes, toolchain/SDK, actual loop script hash and ABBA order are in provenance.json. Candidate was compiled from the subsequently committed production source; no generated git version stamp exists in these crates. Later evidence-only commits do not change that production tree.

The actual f9f Vite preload helper scans the live link collection backwards for each new dependency. Its source hash is retained; the reduced workload follows that loop and setter/append sequence with synthetic absolute JavaScript dependencies. A separate counted document records 43,071 href reads and 294 href writes for 294 links, plus 294 writes each to rel/as/crossOrigin and zero rel reads. Timed documents use original getters, not the counting wrappers. This distinguishes href reads from total HTMLLinkElement native callback counts; the earlier 43,355 class-counter attribution is not an exact href count.

|Fresh process/order|294 links median ms|600 links median ms|
|---|---:|---:|
|Main A1|64.446|264.926|
|Candidate B1|48.992|201.103|
|Candidate B2|49.424|202.212|
|Main A2|64.464|266.053|

Each cell is six fresh-document scans after one excluded warmup, within its own browser process. The repeated approximately 24% improvement is for this synchronous JS/native loop, not overall browser CPU or page loading. Server request counts and all observations are retained. This reduced all-JavaScript dependency case does not model CSS loading, duplicate dependencies or the full application.

The native change retains one parsed base per PageDom, keyed by the actual node-owner-document base string. Attribute values are read and resolved afresh. No resolved href, node, collection or JS object is retained. Source and canonical base strings are each limited to 4096 bytes for caching; larger bases remain supported through the uncached path. Base/URL mutation and adoption do not rely on DOM generation stamps. Replacement/disposal clear the entry. No full HTML base-element implementation is added.

## Matched profile diagnostic

Same host, same unmodified TOS f9fde4aa0d fixture, backend-token localStorage auth and documented direct-lane version gate bypass. Each binary ran one fresh process with three hard navigations in one context/page, shared64MiB/private64MiB caches. All seven original profile assertions passed on every navigation. The observer-free collector is unchanged except output paths; no app, auth, assertion or routing manifest edits. This is an external diagnostic workload, not an unchanged full-suite run. NativePG run databases were fresh clones of the same-day recorded fixture template, and both runs cleaned to zero run databases while retaining the three ledger templates.

|Navigation|Main usable ms|Candidate usable ms|Main browser CPU to usable ms|Candidate CPU ms|Main RSS MiB|Candidate RSS MiB|
|---|---:|---:|---:|---:|---:|---:|
|1 fresh process/context|625.56|582.65|876.53|827.05|213.45|210.77|
|2 reused context|359.62|344.66|589.50|587.81|298.64|298.80|
|3 reused context|361.37|352.52|637.19|617.29|382.22|385.17|

Both have 293 observed requests on each navigation. These are a single sequential baseline/candidate pair; host/JIT/scheduling variation prevents attributing the full fresh-page difference to this cache. Warm-page usable differences are about 15 and 9 ms, and CPU differences are uneven. Do not pool cold/fresh and warm/reused observations or claim a broad load-speed/CPU/memory improvement. RSS includes retained process memory without forced GC and is not a leak measurement.

Usable time is the first completion of all original sequential assertions, including Playwright polling and host scheduling. DCL/load, absolute navigation epochs, process sampling windows, request types and byte observations are in the raw sanitized JSON and summary.json. The network horizon is scheduled one second after usability; actual horizon and last observed asset within it are reported, not a blocking last-asset deadline. CDP dataReceived lengths count decoded bytes here; terminal encoded-body bytes are separate and exclude HTTP headers. fromDiskCache denotes in-memory caching in this implementation. CPU samples include all browser threads and can exceed wall time. DB/setup/auth costs are excluded from navigation windows; original setup timing logs remain retained with hashes.

Release DOM tests cover current attribute/base/URL changes, inherited blank/srcdoc bases, same-VM isolated worlds, adoption, replacement/disposal and oversized/canonicalized fallback. All197 affected WebAPI release tests pass, including137 DOM bindings; strict release all-target clippy and formatting pass. Regeneration writes only summary.json and leaves this narrative intact.
