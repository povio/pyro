// page.coverage: JS and CSS coverage in Playwright's (V8's) entry format, read
// over CDP as Playwright does on Chromium: Profiler precise coverage with the
// scripts from Debugger.scriptParsed, CSS rule usage with the sheets from
// CSS.styleSheetAdded. The engine counts JavaScript blocks only while JS
// coverage is on (JavaScriptCore's control flow profiler; switching it
// recompiles the page's code once each way).
import type { Cdp } from '../cdp.js';
import type { Page } from './page.js';

export type JSCoverageEntry = {
  url: string;
  scriptId: string;
  source?: string;
  functions: Array<{ functionName: string; isBlockCoverage: boolean; ranges: Array<{ count: number; startOffset: number; endOffset: number }> }>;
};
export type CSSCoverageEntry = { url: string; text?: string; ranges: Array<{ start: number; end: number }> };

/** What Coverage needs of its page: its CDP connection and session. */
type PageWire = {
  _cdp: Cdp;
  _sessionId: string;
  /** What the page undoes when it closes. */
  _off: (() => void)[];
  _send<T = any>(method: string, params?: object): Promise<T>;
};

/** Listens to one page session's events until `off()` or the page closes. */
function listen(page: PageWire, handlers: Record<string, (params: any) => void>): () => void {
  const offs = Object.entries(handlers).map(([method, fn]) => page._cdp.on(method, (params, sid) => sid === page._sessionId && fn(params)));
  const off = () => {
    offs.forEach((o) => o());
    const i = page._off.indexOf(off);
    if (i >= 0) page._off.splice(i, 1);
  };
  page._off.push(off);
  return off;
}

class JSCoverage {
  private _enabled = false;
  private _scriptIds = new Set<string>();
  private _scriptSources = new Map<string, string>();
  private _sourceRequests: Promise<void>[] = [];
  private _resetOnNavigation = true;
  private _reportAnonymousScripts = false;
  private _off?: () => void;

  constructor(private readonly _page: PageWire) {}

  async start(options: { reportAnonymousScripts?: boolean; resetOnNavigation?: boolean }): Promise<void> {
    if (this._enabled) throw new Error('JSCoverage is already enabled');
    this._resetOnNavigation = options.resetOnNavigation ?? true;
    this._reportAnonymousScripts = !!options.reportAnonymousScripts;
    this._enabled = true;
    this._scriptIds.clear();
    this._scriptSources.clear();
    this._sourceRequests = [];
    this._off = listen(this._page, {
      'Debugger.scriptParsed': (p) => this._onScriptParsed(p),
      'Debugger.paused': () => void this._page._send('Debugger.resume').catch(() => {}),
      // A new document of the main frame (Chrome clears its execution contexts).
      'Page.frameNavigated': (p) => {
        if (!p.frame.parentId && this._resetOnNavigation) this._reset();
      },
    });
    try {
      await Promise.all([
        this._page._send('Profiler.enable'),
        this._page._send('Profiler.startPreciseCoverage', { callCount: true, detailed: true }),
        this._page._send('Debugger.enable'),
        this._page._send('Debugger.setSkipAllPauses', { skip: true }),
      ]);
    } catch (e) {
      this._off();
      this._enabled = false;
      throw e;
    }
  }

  private _reset() {
    this._scriptIds.clear();
    this._scriptSources.clear();
  }

  private _onScriptParsed(p: { scriptId: string; url: string }) {
    this._scriptIds.add(p.scriptId);
    // Anonymous scripts only when asked for.
    if (!p.url && !this._reportAnonymousScripts) return;
    this._sourceRequests.push(
      this._page._send<{ scriptSource: string }>('Debugger.getScriptSource', { scriptId: p.scriptId }).then(
        (r) => void (this._scriptIds.has(p.scriptId) && this._scriptSources.set(p.scriptId, r.scriptSource)),
        () => {},
      ),
    );
  }

  async stop(): Promise<JSCoverageEntry[]> {
    // As Playwright: nothing when coverage is not on.
    if (!this._enabled) return [];
    this._enabled = false;
    let profile: { result: JSCoverageEntry[] };
    try {
      // The engine reports a script (`Debugger.scriptParsed`) when it next
      // flushes, at the latest just before the take's reply: right after a
      // `goto` the source requests go out only then. Stopping the profiler
      // drops the script table, so the sources are read first.
      profile = await this._page._send<{ result: JSCoverageEntry[] }>('Profiler.takePreciseCoverage');
      await Promise.all(this._sourceRequests);
      await Promise.all([this._page._send('Profiler.stopPreciseCoverage'), this._page._send('Profiler.disable')]);
      await this._page._send('Debugger.disable').catch(() => {});
    } finally {
      this._off?.();
      this._off = undefined;
    }
    const entries: JSCoverageEntry[] = [];
    for (const entry of profile.result) {
      if (!this._scriptIds.has(entry.scriptId)) continue;
      if (!entry.url && !this._reportAnonymousScripts) continue;
      const source = this._scriptSources.get(entry.scriptId);
      entries.push(source === undefined ? entry : { ...entry, source });
    }
    return entries;
  }
}

class CSSCoverage {
  private _enabled = false;
  private _urls = new Map<string, string>();
  private _texts = new Map<string, string>();
  private _textRequests: Promise<void>[] = [];
  private _resetOnNavigation = true;
  private _off?: () => void;

  constructor(private readonly _page: PageWire) {}

  async start(options: { resetOnNavigation?: boolean }): Promise<void> {
    if (this._enabled) throw new Error('CSSCoverage is already enabled');
    this._resetOnNavigation = options.resetOnNavigation ?? true;
    this._enabled = true;
    this._urls.clear();
    this._texts.clear();
    this._textRequests = [];
    this._off = listen(this._page, {
      'CSS.styleSheetAdded': (p) => this._onStyleSheet(p.header),
      'Page.frameNavigated': (p) => {
        if (!p.frame.parentId && this._resetOnNavigation) {
          this._urls.clear();
          this._texts.clear();
        }
      },
    });
    try {
      await Promise.all([this._page._send('CSS.enable'), this._page._send('CSS.startRuleUsageTracking')]);
    } catch (e) {
      this._off();
      this._enabled = false;
      throw e;
    }
  }

  private _onStyleSheet(header: { styleSheetId: string; sourceURL: string }) {
    // Sheets without a URL (constructed ones) are not reported.
    if (!header.sourceURL) return;
    this._textRequests.push(
      this._page._send<{ text: string }>('CSS.getStyleSheetText', { styleSheetId: header.styleSheetId }).then(
        (r) => {
          this._urls.set(header.styleSheetId, header.sourceURL);
          this._texts.set(header.styleSheetId, r.text);
        },
        () => {},
      ),
    );
  }

  async stop(): Promise<CSSCoverageEntry[]> {
    // As Playwright: nothing when coverage is not on.
    if (!this._enabled) return [];
    this._enabled = false;
    let usage: { ruleUsage: Array<{ styleSheetId: string; startOffset: number; endOffset: number; used: boolean }> };
    try {
      usage = await this._page._send('CSS.stopRuleUsageTracking');
      await Promise.all(this._textRequests);
      await this._page._send('CSS.disable').catch(() => {});
    } finally {
      this._off?.();
      this._off = undefined;
    }
    const used = new Map<string, Array<{ start: number; end: number }>>();
    for (const rule of usage.ruleUsage) {
      if (!rule.used) continue;
      let ranges = used.get(rule.styleSheetId);
      if (!ranges) used.set(rule.styleSheetId, (ranges = []));
      ranges.push({ start: rule.startOffset, end: rule.endOffset });
    }
    const entries: CSSCoverageEntry[] = [];
    for (const [id, url] of this._urls) entries.push({ url, text: this._texts.get(id), ranges: disjoint(used.get(id) ?? []) });
    return entries;
  }
}

/** Used ranges sorted and merged where they overlap or touch; single characters dropped. */
function disjoint(ranges: Array<{ start: number; end: number }>): Array<{ start: number; end: number }> {
  const out: Array<{ start: number; end: number }> = [];
  for (const r of [...ranges].sort((a, b) => a.start - b.start || a.end - b.end)) {
    const last = out[out.length - 1];
    if (last && r.start <= last.end) last.end = Math.max(last.end, r.end);
    else out.push({ ...r });
  }
  return out.filter((r) => r.end - r.start > 1);
}

export class Coverage {
  private readonly _js: JSCoverage;
  private readonly _css: CSSCoverage;

  constructor(page: Page) {
    const wire = page as unknown as PageWire;
    this._js = new JSCoverage(wire);
    this._css = new CSSCoverage(wire);
  }

  /** Starts JS coverage. `resetOnNavigation` (default true) drops the scripts of earlier documents; anonymous scripts (eval, `new Function`) are reported with `reportAnonymousScripts`. */
  startJSCoverage(options: { reportAnonymousScripts?: boolean; resetOnNavigation?: boolean } = {}): Promise<void> {
    return this._js.start(options);
  }
  /** Stops JS coverage: per script its V8 block coverage, with its source. */
  stopJSCoverage(): Promise<JSCoverageEntry[]> {
    return this._js.stop();
  }
  /** Starts CSS coverage. `resetOnNavigation` (default true) drops the sheets of earlier documents. */
  startCSSCoverage(options: { resetOnNavigation?: boolean } = {}): Promise<void> {
    return this._css.start(options);
  }
  /** Stops CSS coverage: per style sheet its text and the ranges of the rules used. */
  stopCSSCoverage(): Promise<CSSCoverageEntry[]> {
    return this._css.stop();
  }
}
