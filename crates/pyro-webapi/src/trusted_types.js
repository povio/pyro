// Trusted Types (https://w3c.github.io/trusted-types/dist/spec/), made on a realm's first use
// (trusted_types.rs): TrustedHTML, TrustedScript, TrustedScriptURL, TrustedTypePolicy,
// TrustedTypePolicyFactory and the realm's `trustedTypes` factory. `native` reports what the
// realm's CSP allows for policy names (`trusted-types` directive); with no such directive every
// name is allowed, more than once.
(native => {
  'use strict';
  const global = globalThis;
  const freeze = Object.freeze;
  const defineProperty = Object.defineProperty;
  const apply = Reflect.apply;
  const String_ = String;
  const illegal = () => new TypeError('Illegal constructor');
  const ILLEGAL = Symbol('illegal');

  // [[Data]] of each Trusted* object, by kind.
  const data = new WeakMap();
  function makeTrusted(Ctor, value) {
    const object = Ctor === TrustedHTML ? native.makeValue('html', Ctor.prototype, value) : new Ctor(ILLEGAL);
    data.set(object, { kind: Ctor, value });
    return object;
  }
  function dataOf(Ctor, self, what) {
    const d = data.get(self);
    if (d === undefined || d.kind !== Ctor) throw new TypeError(`Failed to execute '${what}' on '${Ctor.name}': Illegal invocation`);
    return d.value;
  }
  function trustedClass(name) {
    const Ctor = {
      [name]: class {
        constructor(token = undefined) { if (token !== ILLEGAL) throw illegal(); }
        toString() { return dataOf(Ctor, this, 'toString'); }
        toJSON() { return dataOf(Ctor, this, 'toJSON'); }
      },
    }[name];
    defineProperty(Ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
    return Ctor;
  }
  const TrustedHTML = trustedClass('TrustedHTML');
  const TrustedScript = trustedClass('TrustedScript');
  const TrustedScriptURL = trustedClass('TrustedScriptURL');
  for (const [Ctor, method] of [[TrustedHTML, 'fromLiteral'], [TrustedScript, 'fromLiteral'], [TrustedScriptURL, 'fromLiteral']]) {
    // Trusted Types: `fromLiteral(templateStringsArray)` (a template literal without substitutions).
    defineProperty(Ctor, method, {
      value: function fromLiteral(templateStringsArray) {
        if (typeof templateStringsArray !== 'object' || templateStringsArray === null || !Object.isFrozen(templateStringsArray)
            || !Array.isArray(templateStringsArray.raw) || templateStringsArray.length !== 1) {
          throw new TypeError(`Failed to execute 'fromLiteral' on '${Ctor.name}': the argument is not a template literal without substitutions.`);
        }
        return makeTrusted(Ctor, String_(templateStringsArray[0]));
      },
      writable: true, configurable: true,
    });
  }

  // The interface has no public attributes. Its internal slots are captured
  // when a policy returns, independently of later edits to that dictionary.
  class TrustedHTMLParserOptions {
    constructor() { throw illegal(); }
  }
  defineProperty(TrustedHTMLParserOptions.prototype, Symbol.toStringTag, { value: 'TrustedHTMLParserOptions', configurable: true });

  function parserDictionary(options) {
    if (options === null || options === undefined) return { runScripts: false };
    if (typeof options !== 'object' && typeof options !== 'function') throw new TypeError('Expected HTML parser options');
    const runScripts = !!options.runScripts;
    if (options.sanitizer !== undefined) throw new TypeError('HTML sanitizer options are unsupported');
    return { runScripts };
  }

  // Each policy: its name and options.
  const policies = new WeakMap();
  const TrustedTypePolicy = class TrustedTypePolicy {
    constructor() { throw illegal(); }
    get name() { return policyOf(this, 'name').name; }
    createHTML(input, ...args) { return create(policyOf(this, 'createHTML'), TrustedHTML, 'createHTML', input, args); }
    createScript(input, ...args) { return create(policyOf(this, 'createScript'), TrustedScript, 'createScript', input, args); }
    createScriptURL(input, ...args) { return create(policyOf(this, 'createScriptURL'), TrustedScriptURL, 'createScriptURL', input, args); }
  };
  if (native.windowRealm) {
    defineProperty(TrustedTypePolicy.prototype, 'createParserOptions', {
      value: function createParserOptions(options = {}, ...args) {
        return native.createParserOptions(this, options, args);
      },
      writable: true, configurable: true, enumerable: true,
    });
  }
  defineProperty(TrustedTypePolicy.prototype, Symbol.toStringTag, { value: 'TrustedTypePolicy', configurable: true });
  function policyOf(self, what) {
    const p = policies.get(self);
    if (p === undefined) throw new TypeError(`Failed to execute '${what}' on 'TrustedTypePolicy': Illegal invocation`);
    return p;
  }
  // "Get Trusted Type policy value" and "create a Trusted Type".
  function policyValue(policy, method, input, args, throwIfMissing) {
    const fn = policy.options[method];
    if (fn === undefined || fn === null) {
      if (throwIfMissing) throw new TypeError(`Failed to execute '${method}' on 'TrustedTypePolicy': Policy ${policy.name}'s TrustedTypePolicyOptions did not specify a '${method}' member.`);
      return null;
    }
    return apply(fn, undefined, [String_(input), ...args]);
  }
  function create(policy, Ctor, method, input, args) {
    const value = policyValue(policy, method, input, args, true);
    return makeTrusted(Ctor, value === null || value === undefined ? '' : String_(value));
  }

  // TrustedTypePolicyOptions: callbacks read once, at creation.
  function options(value) {
    if (value === undefined || value === null) return { __proto__: null };
    if (typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeError("Failed to execute 'createPolicy' on 'TrustedTypePolicyFactory': The provided value is not of type 'TrustedTypePolicyOptions'.");
    }
    const out = { __proto__: null };
    for (const key of ['createHTML', 'createParserOptions', 'createScript', 'createScriptURL']) {
      const fn = value[key];
      if (fn !== undefined && fn !== null && typeof fn !== 'function') {
        throw new TypeError(`Failed to execute 'createPolicy' on 'TrustedTypePolicyFactory': The '${key}' property is not a function.`);
      }
      out[key] = fn ?? undefined;
    }
    return out;
  }

  const created = [];
  let defaultPolicy = null;
  let defaultParserCallback;
  let emptyHTML = null;
  let emptyScript = null;
  const TrustedTypePolicyFactory = class TrustedTypePolicyFactory {
    constructor(token = undefined) { if (token !== ILLEGAL) throw illegal(); }
    createPolicy(policyName, policyOptions) {
      self_(this, 'createPolicy');
      if (arguments.length < 1) throw new TypeError("Failed to execute 'createPolicy' on 'TrustedTypePolicyFactory': 1 argument required, but only 0 present.");
      const name = String_(policyName);
      const callbacks = options(policyOptions);
      // CSP "Should Trusted Type policy creation be blocked by Content Security Policy?"
      const blocked = policyCreationBlocked(name, created.includes(name));
      if (blocked) {
        throw new TypeError(`Failed to execute 'createPolicy' on 'TrustedTypePolicyFactory': Policy "${name}" disallowed.`);
      }
      const callback = callbacks.createParserOptions;
      const createOptions = (options, args) => {
        const input = parserDictionary(options);
        if (callback === undefined) throw new TypeError('This policy does not define createParserOptions');
        const result = callback(input, ...args);
        if (result === null || result === undefined) throw new TypeError('Expected parser options from policy');
        return result;
      };
      const policy = native.makeValue('policy', TrustedTypePolicy.prototype, createOptions, TrustedHTMLParserOptions.prototype);
      policies.set(policy, { name, options: callbacks });
      created.push(name);
      if (name === 'default') {
        defaultPolicy = policy;
        defaultParserCallback = callback;
      }
      return policy;
    }
    isHTML(value) { self_(this, 'isHTML'); return data.get(value)?.kind === TrustedHTML; }
    isScript(value) { self_(this, 'isScript'); return data.get(value)?.kind === TrustedScript; }
    isScriptURL(value) { self_(this, 'isScriptURL'); return data.get(value)?.kind === TrustedScriptURL; }
    get emptyHTML() { self_(this, 'emptyHTML'); return emptyHTML ??= freeze(makeTrusted(TrustedHTML, '')); }
    get emptyScript() { self_(this, 'emptyScript'); return emptyScript ??= freeze(makeTrusted(TrustedScript, '')); }
    get defaultPolicy() { self_(this, 'defaultPolicy'); return defaultPolicy; }
    getAttributeType(tagName, attribute, elementNs = '', attrNs = '') {
      self_(this, 'getAttributeType');
      return attributeType(String_(tagName).toLowerCase(), String_(attribute).toLowerCase(), nsOf(elementNs), nsOf(attrNs));
    }
    getPropertyType(tagName, property, elementNs = '') {
      self_(this, 'getPropertyType');
      return propertyType(String_(tagName).toLowerCase(), String_(property), nsOf(elementNs));
    }
  };
  defineProperty(TrustedTypePolicyFactory.prototype, Symbol.toStringTag, { value: 'TrustedTypePolicyFactory', configurable: true });
  const factory = Reflect.construct(TrustedTypePolicyFactory, [ILLEGAL], TrustedTypePolicyFactory);
  function self_(value, what) {
    if (value !== factory) throw new TypeError(`Failed to execute '${what}' on 'TrustedTypePolicyFactory': Illegal invocation`);
  }

  const HTML_NS = 'http://www.w3.org/1999/xhtml';
  const SVG_NS = 'http://www.w3.org/2000/svg';
  const XLINK_NS = 'http://www.w3.org/1999/xlink';
  // An empty or null namespace is the HTML namespace for elements (the spec's default).
  const nsOf = ns => (ns === undefined || ns === null || ns === '' ? '' : String_(ns));
  // "Get Trusted Type data for attribute": event handler content attributes are TrustedScript;
  // the table: iframe srcdoc (TrustedHTML), HTML script src and SVG script href (TrustedScriptURL).
  function attributeType(tag, attribute, elementNs, attrNs) {
    const elNs = elementNs === '' ? HTML_NS : elementNs;
    if (attrNs === '' && attribute.startsWith('on') && attribute.length > 2) return 'TrustedScript';
    if (elNs === HTML_NS && attrNs === '' && tag === 'iframe' && attribute === 'srcdoc') return 'TrustedHTML';
    if (elNs === HTML_NS && attrNs === '' && tag === 'script' && attribute === 'src') return 'TrustedScriptURL';
    if (elNs === SVG_NS && tag === 'script' && attribute === 'href' && (attrNs === '' || attrNs === XLINK_NS)) return 'TrustedScriptURL';
    return null;
  }
  // "Get Trusted Type data for property".
  function propertyType(tag, property, elementNs) {
    const elNs = elementNs === '' ? HTML_NS : elementNs;
    if (elNs === HTML_NS) {
      if (tag === 'iframe' && property === 'srcdoc') return 'TrustedHTML';
      if (tag === 'script' && (property === 'innerText' || property === 'textContent' || property === 'text')) return 'TrustedScript';
      if (tag === 'script' && property === 'src') return 'TrustedScriptURL';
    }
    if (property === 'innerHTML' || property === 'outerHTML') return 'TrustedHTML';
    return null;
  }

  // CSP violation reports (CSP 3 "report a violation", for the Trusted Types directives only):
  // a SecurityPolicyViolationEvent fired at the document (the worker global in a worker), in a
  // task, bubbling and composed.
  const EventBase = Event;
  const violationFields = ['documentURI', 'referrer', 'blockedURI', 'effectiveDirective', 'violatedDirective',
    'originalPolicy', 'sourceFile', 'sample', 'disposition', 'statusCode', 'lineNumber', 'columnNumber'];
  const violationData = new WeakMap();
  class SecurityPolicyViolationEvent extends EventBase {
    constructor(type, init) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'SecurityPolicyViolationEvent': 1 argument required, but only 0 present.");
      super(type, init);
      const i = init ?? {};
      const d = {};
      for (const f of violationFields) {
        if (f === 'statusCode' || f === 'lineNumber' || f === 'columnNumber') d[f] = Number(i[f] ?? 0) >>> 0;
        else if (f === 'disposition') d[f] = i[f] === 'report' ? 'report' : 'enforce';
        else d[f] = i[f] === undefined ? '' : String_(i[f]);
      }
      if (i.violatedDirective === undefined && i.effectiveDirective !== undefined) d.violatedDirective = d.effectiveDirective;
      violationData.set(this, d);
    }
  }
  for (const f of violationFields) {
    defineProperty(SecurityPolicyViolationEvent.prototype, f, {
      get() {
        const d = violationData.get(this);
        if (d === undefined) throw new TypeError(`Illegal invocation: SecurityPolicyViolationEvent.${f}`);
        return d[f];
      },
      enumerable: true, configurable: true,
    });
  }
  defineProperty(SecurityPolicyViolationEvent.prototype, Symbol.toStringTag, { value: 'SecurityPolicyViolationEvent', configurable: true });
  const setTimeoutFn = global.setTimeout;
  function report(policy, directive, blockedURI, sample) {
    const target = typeof global.document === 'object' && global.document !== null ? global.document : global;
    const init = {
      bubbles: true, composed: true,
      documentURI: target === global ? String_(global.location?.href ?? '') : String_(target.URL ?? ''),
      referrer: target === global ? '' : String_(target.referrer ?? ''),
      blockedURI, effectiveDirective: directive, violatedDirective: directive,
      originalPolicy: policy.text, sample: String_(sample),
      disposition: policy.reportOnly ? 'report' : 'enforce', statusCode: 0,
    };
    apply(setTimeoutFn, global, [() => target.dispatchEvent(new SecurityPolicyViolationEvent('securitypolicyviolation', init)), 0]);
  }
  function cspPolicies() {
    return native.policies().map(([text, reportOnly]) => ({ text, reportOnly, directives: directivesOf(text) }));
  }
  function directivesOf(text) {
    const out = new Map();
    for (const part of text.split(';')) {
      const tokens = part.trim().split(/[\t\n\f\r ]+/).filter(Boolean);
      if (tokens.length === 0) continue;
      const name = tokens[0].toLowerCase();
      if (!out.has(name)) out.set(name, tokens.slice(1));
    }
    return out;
  }
  // "Should Trusted Type policy creation be blocked": each policy's `trusted-types` directive must
  // list the name (or `*`) and, for a name already created, `'allow-duplicates'`.
  function policyCreationBlocked(name, duplicate) {
    let blocked = false;
    for (const policy of cspPolicies()) {
      const values = policy.directives.get('trusted-types');
      if (values === undefined) continue;
      const lower = values.map(v => v.toLowerCase());
      const violates = (duplicate && !lower.includes("'allow-duplicates'")) || !values.some(v => v === '*' || v === name);
      if (violates) {
        report(policy, 'trusted-types', 'trusted-types-policy', name.slice(0, 40));
        if (!policy.reportOnly) blocked = true;
      }
    }
    return blocked;
  }

  const exports = {
    TrustedHTML, TrustedScript, TrustedScriptURL, TrustedHTMLParserOptions, TrustedTypePolicy, TrustedTypePolicyFactory, SecurityPolicyViolationEvent,
    // A violation of another directive (csp.rs, already in its own task): fired at `target` (a
    // connected element), else the document, else the global (a worker).
    dispatchViolation(target, init) {
      const doc = typeof global.document === 'object' && global.document !== null ? global.document : null;
      const at = target ?? doc ?? global;
      const full = {
        bubbles: true, composed: true,
        documentURI: doc === null ? String_(global.location?.href ?? '') : String_(doc.URL ?? ''),
        referrer: doc === null ? '' : String_(doc.referrer ?? ''),
      };
      for (const key of Object.keys(init)) full[key] = init[key];
      at.dispatchEvent(new SecurityPolicyViolationEvent('securitypolicyviolation', full));
    },
    defaultParserOptions(runScripts, sink, hasSanitizer) {
      const callback = defaultParserCallback;
      if (callback === undefined) return undefined;
      if (hasSanitizer) throw new TypeError('Sanitizer objects for policy callback input are unsupported');
      const result = callback({ runScripts }, 'TrustedHTMLParserOptions', sink);
      if (result === null || result === undefined) return null;
      return result;
    },
    // A sink violation ("Should sink type mismatch violation be blocked"): reported to each
    // policy with `require-trusted-types-for 'script'`; whether one of them is enforced.
    sinkViolation(sink, input) {
      let enforced = false;
      for (const policy of cspPolicies()) {
        const values = policy.directives.get('require-trusted-types-for');
        if (values === undefined || !values.includes("'script'")) continue;
        report(policy, 'require-trusted-types-for', 'trusted-types-sink', `${sink}|${String_(input).slice(0, 40)}`);
        if (!policy.reportOnly) enforced = true;
      }
      return enforced;
    },
    trustedTypes: factory,
    // For native sinks (slice 2): the trusted value's text, or null.
    trustedData(kind, value) {
      const d = data.get(value);
      const Ctor = kind === 'html' ? TrustedHTML : kind === 'script' ? TrustedScript : TrustedScriptURL;
      return d !== undefined && d.kind === Ctor ? d.value : null;
    },
    // JSC patch 0035, eval and Function: a TrustedScript passed to eval is its text.
    codeForEval(value) {
      const d = data.get(value);
      return d !== undefined && d.kind === TrustedScript ? d.value : null;
    },
    // Trusted Types' HostEnsureCanCompileStrings: whether `code` may compile, for `kind`
    // "Function" (with the constructor's arguments), "eval" or "indirect-eval". A Function
    // made of TrustedScripts only passes; else the default policy's createScript must give the
    // code back unchanged, or the violation is reported (and blocks under an enforced policy).
    canCompile(kind, code, ...args) {
      const sink = kind === 'Function' ? 'Function' : 'eval';
      if (kind === 'Function' && args.length > 0 && args.every((a) => {
        const d = data.get(a);
        return d !== undefined && d.kind === TrustedScript;
      })) return true;
      let value;
      if (defaultPolicy !== null) value = policyValue(policies.get(defaultPolicy), 'createScript', code, ['TrustedScript', sink], false);
      if (value === undefined || value === null) {
        let enforced = false;
        for (const policy of cspPolicies()) {
          const values = policy.directives.get('require-trusted-types-for');
          if (values === undefined || !values.includes("'script'")) continue;
          report(policy, 'require-trusted-types-for', 'trusted-types-sink', `${sink}|${String_(code).slice(0, 40)}`);
          if (!policy.reportOnly) enforced = true;
        }
        return !enforced;
      }
      return String_(value) === code;
    },
    // The default policy's value for a sink, called with the trusted type's name and the sink
    // ("get Trusted Type compliant string"); undefined without a default policy, null when it
    // has no such member.
    defaultPolicyValue(method, input, typeName, sink) {
      if (defaultPolicy === null) return undefined;
      return policyValue(policies.get(defaultPolicy), method, input, [typeName, sink], false);
    },
  };
  // eval and Function go through these (JSC patch 0035; a no-op without it).
  native.installCompileHooks(exports.codeForEval, exports.canCompile);
  return exports;
})
