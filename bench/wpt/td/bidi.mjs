// WebDriver BiDi commands that Pyro's CDP really carries out: log.entryAdded (Runtime.consoleAPICalled and
// exceptionThrown), permissions.setPermission (Browser.setPermission), and the
// emulation overrides with a CDP method behind them that the engine evaluates: geolocation
// (Emulation.setGeolocationOverride), locale (setLocaleOverride), touch (setTouchEmulationEnabled) and the media
// features the engine's media evaluation has (setEmulatedMedia: prefers-color-scheme, prefers-reduced-motion). The
// other emulation overrides (screen orientation, scrollbar type, viewport meta, client hints) and speculation are not
// registered: the engine has nothing behind them.

// The pages a command's `contexts` names: none/null is the calling page.
async function targets(ctx, params) {
  const list = params?.contexts;
  if (list == null) return [ctx.page];
  if (!Array.isArray(list) || !list.length) throw new Error('invalid argument: contexts must be a non-empty array');
  const pages = [];
  for (const w of list) {
    if (!w || typeof w !== 'object' || !w.window) throw new Error('invalid argument: a context must be a window');
    const page = await ctx.windowPage(w);
    if (!page) throw new Error('no such frame');
    pages.push(page);
  }
  return pages;
}

// ---- log.entryAdded

// BiDi's console formatter (the console spec's Formatter on a first string argument with %-specifiers) over the
// arguments as CDP describes them: `%s %d %i %f %o %O %c %%`.
function formatText(args) {
  const text = (a) => (a.type === 'string' ? a.value : a.unserializableValue ?? (a.value !== undefined ? String(a.value) : a.description ?? a.type));
  const [first, ...rest] = args;
  if (!first || first.type !== 'string' || !first.value.includes('%')) return args.map(text).join(' ');
  let i = 0;
  // `%d`, `%i`, `%f` of a symbol (and of an object) are NaN, of a bigint its value.
  const num = (a) => (a.type === 'number' ? Number(a.unserializableValue ?? a.value) : a.type === 'boolean' || a.type === 'string' ? Number(a.value) : NaN);
  const out = first.value.replace(/%([sdifoOc%])/g, (m, f) => {
    if (f === '%') return '%';
    if (i >= rest.length) return m;
    const a = rest[i++];
    switch (f) {
      case 'd': return a.type === 'bigint' ? text(a) : String(num(a));
      case 'i': return a.type === 'bigint' ? text(a) : String(Math.trunc(num(a)));
      case 'f': return a.type === 'bigint' ? text(a) : String(num(a));
      case 'c': return '';
      default: return text(a);
    }
  });
  return [out, ...rest.slice(i).map(text)].join(' ');
}

// A CDP RemoteObject as a BiDi remote value (`script.RemoteValue`, the part tests read: type and primitive value).
function remoteValue(a) {
  switch (a.type) {
    case 'string': return { type: 'string', value: a.value };
    case 'number': return { type: 'number', value: a.unserializableValue ?? a.value };
    case 'boolean': return { type: 'boolean', value: a.value };
    case 'bigint': return { type: 'bigint', value: String(a.unserializableValue ?? a.description).replace(/n$/, '') };
    case 'undefined': return { type: 'undefined' };
    case 'symbol': return { type: 'symbol' };
    case 'function': return { type: 'function' };
    default: return a.subtype === 'null' ? { type: 'null' } : { type: a.subtype === 'array' ? 'array' : 'object' };
  }
}

const METHOD = { warning: 'warn', startGroup: 'group', startGroupCollapsed: 'groupCollapsed', endGroup: 'groupEnd' };
const LEVEL = { error: 'error', assert: 'error', warning: 'warn', debug: 'debug' };

// Subscriptions of a browser context: `{ id, frame, pages: Set | null }` (null: every page), and the pages whose CDP
// session already feeds the table. The console is read from each page's own session (Runtime.consoleAPICalled and
// exceptionThrown): it works for popups, where Playwright's console events never arrive.
function table(ctx) {
  return ctx.shared('bidi.log', () => {
    const t = { subs: new Map(), next: 1, chain: Promise.resolve(), seq: 0, attached: new Set(), ctxId: new Map() };
    const where = (page, executionContextId) => ({ realm: String(executionContextId ?? ''), context: String(t.ctxId.get(page)) });
    const deliver = (page, entry) => {
      // Entries are delivered in order, one at a time.
      t.chain = t.chain.then(async () => {
        const subs = [...t.subs.values()].filter((s) => !s.pages || s.pages.has(page));
        const id = ++t.seq;
        for (const s of subs) await ctx.emit(s.frame, 'log.entry_added', { id, entry });
      }).catch(() => {});
    };
    t.attach = async (page) => {
      if (t.attached.has(page)) return;
      t.attached.add(page);
      t.ctxId.set(page, t.ctxId.size + 1);
      const s = await ctx.sessionOf(page);
      s.on('Runtime.consoleAPICalled', (e) => {
        const args = e.args ?? [];
        deliver(page, {
          type: 'console', method: METHOD[e.type] ?? e.type, level: LEVEL[e.type] ?? 'info', source: where(page, e.executionContextId),
          text: formatText(args), timestamp: Math.round(e.timestamp), args: args.map(remoteValue),
        });
      });
      s.on('Runtime.exceptionThrown', (e) => {
        const d = e.exceptionDetails;
        deliver(page, {
          type: 'javascript', level: 'error', source: where(page, d.executionContextId), timestamp: Math.round(e.timestamp),
          text: d.exception?.description ?? d.text, stackTrace: { callFrames: [] },
        });
      });
      await s.send('Runtime.enable');
    };
    // A global subscription (`contexts: null`) also covers the pages opened later.
    ctx.context.on('page', (page) => { if ([...t.subs.values()].some((x) => !x.pages)) t.attach(page).catch(() => {}); });
    return t;
  });
}

const logCommands = {
  async 'bidi.log.entry_added.subscribe'(ctx, [params]) {
    const t = table(ctx);
    // `contexts` omitted: the calling window only. null: every window.
    const pages = params && params.contexts === null ? null : new Set(await targets(ctx, params));
    // Every window: a popup the page opened a moment ago reaches Playwright's page list asynchronously; let the list settle.
    if (!pages) for (let n = -1, stable = 0; stable < 4;) { const now = ctx.context.pages().length; stable = now === n ? stable + 1 : 0; n = now; await new Promise((r) => setTimeout(r, 25)); }
    const id = t.next++;
    t.subs.set(id, { id, frame: ctx.frame, pages });
    // A popup that was closed may still be listed and never answer: give each page a second.
    await Promise.all([...(pages ?? ctx.context.pages())].map((page) => Promise.race([t.attach(page), new Promise((r) => setTimeout(r, 1000))]).catch(() => {})));
    return { id };
  },
  async 'bidi.log.entry_added.unsubscribe'(ctx, [id]) {
    table(ctx).subs.delete(id);
  },
};

// ---- permissions.setPermission

const permissionCommands = {
  async 'bidi.permissions.set_permission'(ctx, [p]) {
    // The permission is Browser.setPermission's, for the top-level origin (the calling window's by default). An embedded
    // origin of its own has no CDP counterpart: refuse it instead of setting the top-level one.
    if (p.embeddedOrigin && p.embeddedOrigin !== (p.origin ?? new URL(ctx.page.url()).origin)) throw new Error('unsupported operation: embeddedOrigin');
    const origin = p.origin ?? new URL(ctx.page.url()).origin;
    await (await ctx.session()).send('Browser.setPermission', { permission: p.descriptor, setting: p.state, origin });
  },
};

// ---- emulation

// Sends `method` to each target page's session (a CDP override of a page, or of its whole context).
async function eachPage(ctx, params, fn) {
  for (const page of await targets(ctx, params)) {
    const session = page === ctx.page ? await ctx.session() : await ctx.context.newCDPSession(page);
    try { await fn(session, page); } finally { if (page !== ctx.page) await session.detach().catch(() => {}); }
  }
}

const emulationCommands = {
  async 'bidi.emulation.set_geolocation_override'(ctx, [p]) {
    const c = p.coordinates;
    await eachPage(ctx, p, async (s) => {
      if (c) {
        await s.send('Emulation.setGeolocationOverride', {
          latitude: c.latitude, longitude: c.longitude, accuracy: c.accuracy ?? 1,
          ...(c.altitude != null ? { altitude: c.altitude } : {}), ...(c.altitudeAccuracy != null ? { altitudeAccuracy: c.altitudeAccuracy } : {}),
          ...(c.heading != null ? { heading: c.heading } : {}), ...(c.speed != null ? { speed: c.speed } : {}),
        });
      } else if (p.error) await s.send('Emulation.setGeolocationOverride', {}); // no position: unavailable
      else await s.send('Emulation.clearGeolocationOverride');
    });
  },
  async 'bidi.emulation.set_locale_override'(ctx, [p]) {
    await eachPage(ctx, p, (s) => s.send('Emulation.setLocaleOverride', p.locale ? { locale: p.locale } : {}));
  },
  async 'bidi.emulation.set_touch_override'(ctx, [p]) {
    await eachPage(ctx, p, (s) => s.send('Emulation.setTouchEmulationEnabled', p.maxTouchPoints != null ? { enabled: true, maxTouchPoints: p.maxTouchPoints } : { enabled: false }));
  },
  async 'bidi.emulation.set_media_features_override'(ctx, [p]) {
    const evaluated = { 'prefers-color-scheme': ['light', 'dark'], 'prefers-reduced-motion': ['reduce', 'no-preference'] };
    for (const [name, value] of Object.entries(p.features ?? {})) {
      if (value === null) continue;
      if (!evaluated[name]) throw new Error(`unsupported operation: the media feature ${name} is not evaluated`);
      if (!evaluated[name].includes(value)) throw new Error(`invalid argument: ${name}: ${value}`);
    }
    await eachPage(ctx, p, async (s, page) => {
      // Emulation.setEmulatedMedia replaces the page's whole emulation: keep what each feature was set to.
      const state = ctx.shared('media', () => new Map());
      const now = p.features == null ? new Map() : new Map(state.get(page) ?? []);
      for (const [name, value] of Object.entries(p.features ?? {})) { if (value === null) now.delete(name); else now.set(name, value); }
      state.set(page, now);
      await s.send('Emulation.setEmulatedMedia', { features: [...now].map(([name, value]) => ({ name, value })) });
    });
  },
};

export const commands = { ...logCommands, ...permissionCommands, ...emulationCommands };
