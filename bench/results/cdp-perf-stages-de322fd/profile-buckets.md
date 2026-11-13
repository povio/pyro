# Profile buckets per workload (tools/xcat2.mjs output)

## getVersion (scaled to 25 µs browser CPU/op)

```
samples 4113, ~6 ns/op per sample
 51.2%    12795 ns/op  plumbing: cross-thread wake/park (pipe, kevent)
 33.8%     8461 ns/op  plumbing: socket syscalls
  5.0%     1246 ns/op  plumbing: tokio runtime
  4.2%     1052 ns/op  cdp: dispatch/target logic
  2.1%      529 ns/op  plumbing: JSON (serde)
  1.7%      432 ns/op  work: DOM/webapi + bindings
  0.9%      231 ns/op  plumbing: websocket framing
  0.4%      109 ns/op  plumbing: channels/outbox
  0.4%       91 ns/op  work: network/fetch
  0.2%       55 ns/op  browser: event-loop checkpoint
  -- top self --
 25.6%     6394 ns/op  kevent
 17.1%     4285 ns/op  __sendto
 13.7%     3422 ns/op  __recvfrom
 11.2%     2808 ns/op  poll
  7.0%     1751 ns/op  write
  3.8%      954 ns/op  read
  2.3%      584 ns/op  mach_absolute_time
  0.5%      122 ns/op  format_escaped_str_contents<&mut alloc::vec::Vec<u8, alloc::alloc::Global>, serde_json::ser::Compact
  0.4%      109 ns/op  clock_gettime_nsec_np
  0.4%      109 ns/op  pthread_mutex_unlock
  0.4%      109 ns/op  {async_fn#0}
  0.4%      103 ns/op  _platform_memmove
  0.4%       97 ns/op  _xzm_free_main
  0.4%       97 ns/op  Handle::task::Schedule::schedule
  0.3%       85 ns/op  pthread_mutex_lock
```

## evaluate (scaled to 31 µs browser CPU/op)

```
samples 4468, ~7 ns/op per sample
 41.8%    12954 ns/op  plumbing: cross-thread wake/park (pipe, kevent)
 25.1%     7778 ns/op  plumbing: socket syscalls
  5.3%     1658 ns/op  engine: JSC execute
  5.2%     1617 ns/op  cdp: dispatch/target logic
  5.1%     1568 ns/op  work: DOM/webapi + bindings
  4.2%     1297 ns/op  plumbing: JSON (serde)
  4.0%     1235 ns/op  browser: event-loop checkpoint
  4.0%     1228 ns/op  plumbing: tokio runtime
  2.0%      624 ns/op  engine: JSC parse/compile/codecache
  1.1%      333 ns/op  engine: JSC watchdog timer
  0.9%      278 ns/op  plumbing: websocket framing
  0.5%      167 ns/op  cdp: RemoteObject describe/register
  0.4%      111 ns/op  plumbing: channels/outbox
  0.2%       76 ns/op  work: network/fetch
  0.2%       56 ns/op  engine: JSC GC
  0.1%       21 ns/op  other
  -- top self --
 21.7%     6730 ns/op  kevent
 13.9%     4323 ns/op  __sendto
 10.0%     3115 ns/op  __recvfrom
  8.6%     2671 ns/op  poll
  5.8%     1797 ns/op  write
  3.0%      916 ns/op  read
  1.9%      604 ns/op  mach_absolute_time
  1.8%      548 ns/op  _xzm_free_main
  1.5%      465 ns/op  _xzm_xzone_malloc_tiny
  0.9%      291 ns/op  _malloc_zone_malloc
  0.9%      278 ns/op  _free
  0.8%      257 ns/op  <deduplicated_symbol>
  0.8%      250 ns/op  _platform_memmove
  0.6%      201 ns/op  __thread_selfusage
  0.6%      187 ns/op  JSC::CodeBlock::CodeBlock(JSC::VM&, JSC::Structure*, JSC::ScriptExecutable*, JSC::UnlinkedCodeBlock*
```

## cfo38 (scaled to 238 µs browser CPU/op)

```
samples 6729, ~35 ns/op per sample
 30.2%    71764 ns/op  engine: JSC parse/compile/codecache
 24.9%    59314 ns/op  plumbing: JSON (serde)
 24.0%    57086 ns/op  engine: JSC execute
  8.1%    19276 ns/op  plumbing: cross-thread wake/park (pipe, kevent)
  5.8%    13688 ns/op  plumbing: socket syscalls
  2.0%     4775 ns/op  plumbing: websocket framing
  1.5%     3572 ns/op  cdp: dispatch/target logic
  1.5%     3572 ns/op  work: DOM/webapi + bindings
  0.8%     1804 ns/op  browser: event-loop checkpoint
  0.7%     1627 ns/op  plumbing: tokio runtime
  0.3%      707 ns/op  engine: JSC watchdog timer
  0.1%      318 ns/op  engine: JSC GC
  0.1%      177 ns/op  cdp: RemoteObject describe/register
  0.1%      141 ns/op  other
  0.0%      106 ns/op  work: network/fetch
  0.0%       71 ns/op  plumbing: channels/outbox
  -- top self --
 19.1%    45343 ns/op  WTF::StringImpl::hashSlowCase() const
 11.3%    26916 ns/op  _platform_memmove
 10.4%    24759 ns/op  JSC::CodeBlock::finishCreation(JSC::VM&, JSC::ScriptExecutable*, JSC::UnlinkedCodeBlock*, JSC::JSSco
  7.4%    17543 ns/op  serde_json::read::SliceRead::skip_to_escape
  3.9%     9373 ns/op  kevent
  3.7%     8772 ns/op  next_code_point<core::slice::iter::Iter<u8>>
  3.2%     7711 ns/op  wrapping_sub
  3.2%     7604 ns/op  set_len<u16, alloc::alloc::Global>
  3.0%     7180 ns/op  __recvfrom
  2.8%     6756 ns/op  capacity<alloc::alloc::Global>
  2.5%     6048 ns/op  __sendto
  1.9%     4527 ns/op  next<u8>
  1.8%     4209 ns/op  read::SliceRead::Read::ignore_str
  1.5%     3608 ns/op  poll
  1.2%     2971 ns/op  write
```

## qsa (scaled to 304 µs browser CPU/op)

```
samples 7229, ~42 ns/op per sample
 70.1%   213124 ns/op  work: selector matching
 13.1%    39782 ns/op  work: DOM/webapi + bindings
  6.2%    18882 ns/op  plumbing: cross-thread wake/park (pipe, kevent)
  4.6%    14088 ns/op  plumbing: socket syscalls
  2.5%     7696 ns/op  engine: JSC execute
  0.8%     2481 ns/op  cdp: dispatch/target logic
  0.7%     2229 ns/op  plumbing: JSON (serde)
  0.5%     1640 ns/op  plumbing: tokio runtime
  0.4%     1304 ns/op  browser: event-loop checkpoint
  0.4%     1177 ns/op  engine: JSC parse/compile/codecache
  0.2%      547 ns/op  engine: JSC watchdog timer
  0.1%      378 ns/op  engine: JSC GC
  0.1%      210 ns/op  cdp: RemoteObject describe/register
  0.1%      168 ns/op  plumbing: channels/outbox
  0.1%      168 ns/op  plumbing: websocket framing
  0.0%       84 ns/op  other
  0.0%       42 ns/op  work: network/fetch
  -- top self --
  9.9%    30110 ns/op  select::dom::Dom::matches_with
  9.9%    30026 ns/op  pyro_dom::select::ElemRef::e
  6.0%    18377 ns/op  as_ref<pyro_dom::node::Node>
  4.8%    14676 ns/op  next<selectors::parser::Component<pyro_dom::select::Impl>>
  4.1%    12532 ns/op  matches_simple_selector<pyro_dom::select::ElemRef>
  3.9%    11985 ns/op  _tlv_get_addr
  2.9%     8873 ns/op  kevent
  2.7%     8200 ns/op  following_in
  2.7%     8074 ns/op  replace<core::option::Option<selectors::parser::Combinator>>
  2.4%     7359 ns/op  __sendto
  2.3%     6897 ns/op  index<pyro_dom::dom::Slot>
  2.2%     6771 ns/op  _platform_memcmp
  2.1%     6518 ns/op  selectors::tree::Element::has_class
  2.1%     6350 ns/op  __recvfrom
  1.9%     5803 ns/op  atomic_load<usize>
```

## mutate (scaled to 3431 µs browser CPU/op)

```
samples 8694, ~395 ns/op per sample
 61.4%  2108167 ns/op  engine: JSC execute
 32.0%  1098678 ns/op  work: DOM/webapi + bindings
  2.6%    89583 ns/op  browser: event-loop checkpoint
  1.3%    43016 ns/op  engine: JSC GC
  0.9%    29203 ns/op  plumbing: socket syscalls
  0.8%    26836 ns/op  plumbing: cross-thread wake/park (pipe, kevent)
  0.4%    14207 ns/op  engine: JSC parse/compile/codecache
  0.2%     7104 ns/op  cdp: dispatch/target logic
  0.2%     5525 ns/op  plumbing: JSON (serde)
  0.1%     3552 ns/op  plumbing: tokio runtime
  0.1%     1973 ns/op  plumbing: websocket framing
  0.1%     1973 ns/op  engine: JSC watchdog timer
  0.0%     1184 ns/op  other
  -- top self --
  9.4%   322421 ns/op  JSC::JSLockHolder::~JSLockHolder()
  3.6%   123917 ns/op  JSC::MicrotaskQueue::drainWithUseCallOnEachMicrotask(JSC::JSGlobalObject*, JSC::VM&, JSC::TopExcepti
  3.1%   105764 ns/op  JSC::VM::setStackPointerAtVMEntry(void*)
  3.0%   102606 ns/op  JSValueProtect
  2.8%    96292 ns/op  _xzm_xzone_malloc_tiny
  2.4%    84058 ns/op  JSC::JSLockHolder::JSLockHolder(JSC::JSGlobalObject*)
  2.3%    80112 ns/op  JSC::JSLockHolder::JSLockHolder(JSC::VM&)
  2.1%    71430 ns/op  mach_absolute_time
  2.1%    71035 ns/op  JSValueUnprotect
  2.1%    70641 ns/op  _xzm_free_main
  1.9%    63537 ns/op  operationValueAddProfiled
  1.5%    52092 ns/op  pyro_webapi::page::PageDom::flush_iframe_events
  1.5%    50119 ns/op  operationGetByIdGaveUp
  1.4%    48146 ns/op  JSC::JSLock::unlock(long)
  1.4%    46962 ns/op  ok<core::option::Option<pyro_js::persistent::Persistent>, pyro_js::error::JsError>
```

## lifecycle (scaled to 2939 µs browser CPU/op)

```
samples 10785, ~273 ns/op per sample
 58.6%  1721706 ns/op  engine: JSC execute
 12.5%   366796 ns/op  work: DOM/webapi + bindings
  5.7%   166775 ns/op  engine: JSC parse/compile/codecache
  5.5%   162960 ns/op  plumbing: cross-thread wake/park (pipe, kevent)
  4.5%   131076 ns/op  plumbing: socket syscalls
  3.8%   110638 ns/op  engine: JSC GC
  3.0%    88838 ns/op  engine: JSC watchdog timer
  2.3%    68127 ns/op  work: network/fetch
  1.0%    30793 ns/op  plumbing: tokio runtime
  1.0%    29976 ns/op  cdp: dispatch/target logic
  0.8%    23981 ns/op  other
  0.5%    13625 ns/op  browser: event-loop checkpoint
  0.4%    11990 ns/op  plumbing: JSON (serde)
  0.2%     6540 ns/op  work: HTML parse
  0.1%     3270 ns/op  plumbing: websocket framing
  0.0%     1363 ns/op  plumbing: channels/outbox
  0.0%      273 ns/op  work: CSS parse
  0.0%      273 ns/op  cdp: RemoteObject describe/register
  -- top self --
  3.3%    95923 ns/op  JSC::DFG::AbstractInterpreter<JSC::DFG::InPlaceAbstractState>::executeEffects(unsigned int, JSC::DFG
  2.7%    80662 ns/op  __thread_selfusage
  2.6%    76847 ns/op  kevent
  2.6%    76030 ns/op  __sendto
  2.0%    58044 ns/op  JSC::JSLockHolder::~JSLockHolder()
  1.7%    51232 ns/op  WTF::ASCIILiteral JSC::JSObject::putDirectInternal<(JSC::JSObject::PutMode)1>(JSC::VM&, JSC::Propert
  1.7%    48779 ns/op  __recvfrom
  1.3%    38696 ns/op  mach_absolute_time
  1.3%    38424 ns/op  pas_thread_local_cache_flush_deallocation_log
  1.3%    36789 ns/op  poll
  1.2%    34609 ns/op  WTF::StringImpl::hashSlowCase() const
  1.2%    34064 ns/op  _platform_memmove
  1.1%    32701 ns/op  JSC::toPropertyDescriptor(JSC::JSGlobalObject*, JSC::JSValue, JSC::PropertyDescriptor&, bool&)
  1.0%    28341 ns/op  JSC::CodeBlock::finishCreation(JSC::VM&, JSC::ScriptExecutable*, JSC::UnlinkedCodeBlock*, JSC::JSSco
  0.9%    26433 ns/op  JSC::JSLockHolder::JSLockHolder(JSC::VM&)
```

## navSimple (scaled to 2641 µs browser CPU/op)

```
samples 11884, ~222 ns/op per sample
 62.0%  1636958 ns/op  engine: JSC execute
 13.6%   360237 ns/op  work: DOM/webapi + bindings
  6.4%   169785 ns/op  engine: JSC parse/compile/codecache
  4.0%   106004 ns/op  engine: JSC GC
  3.4%    89559 ns/op  plumbing: socket syscalls
  3.1%    82226 ns/op  engine: JSC watchdog timer
  3.0%    78003 ns/op  plumbing: cross-thread wake/park (pipe, kevent)
  1.9%    49113 ns/op  work: network/fetch
  0.6%    17112 ns/op  cdp: dispatch/target logic
  0.6%    15334 ns/op  plumbing: JSON (serde)
  0.5%    13556 ns/op  plumbing: tokio runtime
  0.4%    10000 ns/op  browser: event-loop checkpoint
  0.3%     8445 ns/op  work: HTML parse
  0.1%     2222 ns/op  plumbing: channels/outbox
  0.1%     1333 ns/op  plumbing: websocket framing
  0.0%      667 ns/op  work: CSS parse
  0.0%      444 ns/op  other
  -- top self --
  3.7%    98449 ns/op  JSC::DFG::AbstractInterpreter<JSC::DFG::InPlaceAbstractState>::executeEffects(unsigned int, JSC::DFG
  2.8%    74670 ns/op  __thread_selfusage
  2.2%    57114 ns/op  __sendto
  1.9%    51335 ns/op  WTF::ASCIILiteral JSC::JSObject::putDirectInternal<(JSC::JSObject::PutMode)1>(JSC::VM&, JSC::Propert
  1.9%    48891 ns/op  JSC::JSLockHolder::~JSLockHolder()
  1.5%    38891 ns/op  pas_thread_local_cache_flush_deallocation_log
  1.4%    36224 ns/op  mach_absolute_time
  1.3%    34224 ns/op  WTF::StringImpl::hashSlowCase() const
  1.3%    33113 ns/op  kevent
  1.2%    32668 ns/op  _platform_memmove
  1.2%    32224 ns/op  JSC::toPropertyDescriptor(JSC::JSGlobalObject*, JSC::JSValue, JSC::PropertyDescriptor&, bool&)
  1.2%    31557 ns/op  JSC::CodeBlock::finishCreation(JSC::VM&, JSC::ScriptExecutable*, JSC::UnlinkedCodeBlock*, JSC::JSSco
  1.2%    30446 ns/op  JSC::Structure::addNewPropertyTransition(JSC::VM&, JSC::Structure*, JSC::PropertyName, unsigned int,
  1.0%    27334 ns/op  JSC::DFG::Plan::compileInThreadImpl()
  1.0%    25557 ns/op  __psynch_cvwait
```

