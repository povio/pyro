// The Reporting API for one realm (a Window's or a worker's), made on its
// first use: `ReportingObserver`, `ReportBody` and `CSPViolationReportBody`
// as interfaces, plus `Report` and `DeprecationReportBody`, which Chrome
// keeps off the global. Native code (reporting.rs) reaches two helpers:
//   queue(type, url, bodyJSON): a report the engine generated (a CSP
//     violation, after its securitypolicyviolation event); `bodyJSON` holds
//     the body's fields.
//   deprecation(id, message, url): a deprecation report for `id`, once per
//     realm. Its source location is the innermost page frame on the stack.
//     Returns the body's JSON (for the document's `default` endpoint), or
//     null when `id` was already reported.
// Reports go to the realm's buffer (the newest 100, for `buffered`
// observers) and to every observer whose `types` take them. Each such
// observer's callback runs in a task after the report was queued, as in
// Chrome; it still runs when takeRecords() emptied its queue in between, and
// after disconnect() for reports queued before it.
(function () {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const defineProperty = O.defineProperty;
  const getOwnPropertyDescriptor = O.getOwnPropertyDescriptor;
  const getOwnPropertyNames = O.getOwnPropertyNames;
  const TypeErrorC = TypeError;
  const ErrorC = Error;
  const ArrayFrom = Array.from;
  const JSONParse = JSON.parse;
  const JSONStringify = JSON.stringify;
  const setTimeoutC = setTimeout;
  const reportErrorC = typeof reportError === 'function' ? reportError : null;
  const MAX_BUFFER = 100;

  const tag = (C, name) => defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  // Web IDL members: enumerable on the prototype.
  const members = (C) => {
    const p = C.prototype;
    for (const k of getOwnPropertyNames(p)) {
      if (k !== 'constructor') defineProperty(p, k, { enumerable: true });
    }
  };
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const branded = (map, self) => {
    const d = map.get(self);
    if (d === undefined) throw new TypeErrorC('Illegal invocation');
    return d;
  };

  // Report bodies. Each body keeps its kind, so one kind's getters reject
  // another's instances.
  const bodyData = new WeakMap();
  class ReportBody {
    constructor(t, kind, fields) {
      if (t !== token) throw illegal('ReportBody');
      bodyData.set(this, { kind, fields });
    }
    toJSON() {
      branded(bodyData, this);
      return {};
    }
  }
  tag(ReportBody, 'ReportBody');
  members(ReportBody);
  // (No constructor arguments in its IDL: length 0, as Chrome.)
  defineProperty(ReportBody, 'length', { value: 0, configurable: true });

  // A body interface: read-only attributes `getters`, toJSON() with `order`
  // (Chrome's serialization order).
  const bodyKind = (C, name, getters, order) => {
    const p = C.prototype;
    const fieldsOf = (self) => {
      const d = branded(bodyData, self);
      if (d.kind !== name) throw new TypeErrorC('Illegal invocation');
      return d.fields;
    };
    for (const key of getters) {
      const get = getOwnPropertyDescriptor({ get [key]() { return fieldsOf(this)[key]; } }, key).get;
      defineProperty(p, key, { get, enumerable: true, configurable: true });
    }
    defineProperty(p, 'toJSON', {
      value: { toJSON() {
        const fields = fieldsOf(this);
        const out = {};
        for (const key of order) out[key] = fields[key];
        return out;
      } }.toJSON,
      writable: true, enumerable: true, configurable: true,
    });
    tag(C, name);
  };

  const CSP_KEYS = ['documentURL', 'referrer', 'blockedURL', 'effectiveDirective', 'originalPolicy', 'sourceFile',
    'sample', 'disposition', 'statusCode', 'lineNumber', 'columnNumber'];
  class CSPViolationReportBody extends ReportBody {
    constructor(t, fields) {
      if (t !== token) throw illegal('CSPViolationReportBody');
      super(t, 'CSPViolationReportBody', fields);
    }
  }
  bodyKind(CSPViolationReportBody, 'CSPViolationReportBody', CSP_KEYS,
    ['sourceFile', 'lineNumber', 'columnNumber', 'documentURL', 'referrer', 'blockedURL', 'effectiveDirective',
      'originalPolicy', 'sample', 'disposition', 'statusCode']);
  defineProperty(CSPViolationReportBody, 'length', { value: 0, configurable: true });

  class DeprecationReportBody extends ReportBody {
    constructor(t, fields) {
      if (t !== token) throw illegal('DeprecationReportBody');
      super(t, 'DeprecationReportBody', fields);
    }
  }
  bodyKind(DeprecationReportBody, 'DeprecationReportBody',
    ['id', 'anticipatedRemoval', 'message', 'sourceFile', 'lineNumber', 'columnNumber'],
    ['sourceFile', 'lineNumber', 'columnNumber', 'id', 'message', 'anticipatedRemoval']);
  // (Off the global in Chrome: its prototype has no constructor to reach the class through.)
  delete DeprecationReportBody.prototype.constructor;

  const reportData = new WeakMap();
  class Report {
    constructor(t, type, url, body) {
      if (t !== token) throw illegal('Report');
      reportData.set(this, { type, url, body });
    }
    get type() {
      return branded(reportData, this).type;
    }
    get url() {
      return branded(reportData, this).url;
    }
    get body() {
      return branded(reportData, this).body;
    }
    toJSON() {
      const d = branded(reportData, this);
      return { type: d.type, url: d.url, body: d.body === null ? null : d.body.toJSON() };
    }
  }
  tag(Report, 'Report');
  members(Report);
  // No interface object in Chrome: the prototype has no `constructor`.
  delete Report.prototype.constructor;

  // Observers in observe() order, the realm's buffer, and the observers whose
  // callback is due.
  const observerData = new WeakMap();
  const registered = [];
  const buffer = [];
  let due = new Set();

  const deliver = () => {
    const observers = due;
    due = new Set();
    for (const observer of observers) {
      const d = observerData.get(observer);
      const records = d.queue;
      d.queue = [];
      try {
        d.callback.call(observer, records, observer);
      } catch (e) {
        if (reportErrorC !== null) reportErrorC(e);
        else setTimeoutC(() => { throw e; }, 0);
      }
    }
  };
  const notify = (observer) => {
    if (due.size === 0) setTimeoutC(deliver, 0);
    due.add(observer);
  };
  const takes = (d, report) => d.types === null || d.types.includes(branded(reportData, report).type);

  const add = (report) => {
    for (const observer of registered) {
      const d = observerData.get(observer);
      if (!takes(d, report)) continue;
      d.queue.push(report);
      notify(observer);
    }
    buffer.push(report);
    if (buffer.length > MAX_BUFFER) buffer.shift();
  };

  const ctorError = (rest) => new TypeErrorC(`Failed to construct 'ReportingObserver': ${rest}`);

  class ReportingObserver {
    constructor(callback, options = {}) {
      if (arguments.length === 0) throw ctorError('1 argument required, but only 0 present.');
      if (typeof callback !== 'function') throw ctorError("parameter 1 is not of type 'Function'.");
      if (options === null) options = {};
      else if (typeof options !== 'object' && typeof options !== 'function') {
        throw ctorError("The provided value is not of type 'ReportingObserverOptions'.");
      }
      // Dictionary members in lexicographic order.
      const buffered = !!options.buffered;
      const raw = options.types;
      let types = null;
      if (raw !== undefined) {
        if (raw === null || (typeof raw !== 'object' && typeof raw !== 'function') ||
            typeof raw[Symbol.iterator] !== 'function') {
          throw ctorError("Failed to read the 'types' property from 'ReportingObserverOptions': The provided value cannot be converted to a sequence.");
        }
        types = ArrayFrom(raw, String);
        // Chrome: an empty list observes every type.
        if (types.length === 0) types = null;
      }
      observerData.set(this, { callback, types, buffered, queue: [] });
    }
    observe() {
      const d = branded(observerData, this);
      if (registered.includes(this)) return;
      registered.push(this);
      if (!d.buffered) return;
      d.buffered = false;
      let any = false;
      for (const report of buffer) {
        if (!takes(d, report)) continue;
        d.queue.push(report);
        any = true;
      }
      if (any) notify(this);
    }
    disconnect() {
      branded(observerData, this);
      const i = registered.indexOf(this);
      if (i >= 0) registered.splice(i, 1);
    }
    takeRecords() {
      const d = branded(observerData, this);
      const records = d.queue;
      d.queue = [];
      return records;
    }
  }
  tag(ReportingObserver, 'ReportingObserver');
  members(ReportingObserver);

  const queue = (type, url, json) => {
    const fields = JSONParse(json);
    const body = type === 'csp-violation' ? new CSPViolationReportBody(token, fields) : null;
    add(new Report(token, String(type), String(url), body));
  };

  // The innermost frame on the stack outside the engine's own scripts.
  const callerLocation = () => {
    const stack = String(new ErrorC().stack || '');
    for (const line of stack.split('\n')) {
      const at = line.indexOf('@');
      const m = /^(.*):(\d+):(\d+)$/.exec(at >= 0 ? line.slice(at + 1) : line);
      if (m === null || m[1].startsWith('pyro://') || m[1] === '') continue;
      return { sourceFile: m[1].split('#')[0], lineNumber: +m[2], columnNumber: +m[3] };
    }
    return null;
  };

  const deprecated = new Set();
  const deprecation = (id, message, url) => {
    id = String(id);
    if (deprecated.has(id)) return null;
    deprecated.add(id);
    // (No page frame on the stack: the document's URL, with no line or column.)
    const at = callerLocation() || { sourceFile: String(url), lineNumber: null, columnNumber: null };
    const fields = {
      id, message: String(message), sourceFile: at.sourceFile, lineNumber: at.lineNumber,
      columnNumber: at.columnNumber, anticipatedRemoval: null,
    };
    const body = new DeprecationReportBody(token, fields);
    add(new Report(token, 'deprecation', String(url), body));
    return JSONStringify(body.toJSON());
  };

  return { ReportingObserver, ReportBody, CSPViolationReportBody, queue, deprecation };
})
