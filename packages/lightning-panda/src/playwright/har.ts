// `routeFromHAR(path, { update: true })`: records the requests pages make
// (from Network events, bodies from `Network.getResponseBody`) and writes a
// HAR 1.2 file when the context closes, in the form `routeFromHAR` replays.
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { urlMatches, type UrlMatch } from './match.js';
import type { Page, Request } from './page.js';

type Options = { url?: UrlMatch; updateContent?: 'embed' | 'attach' | 'omit' | string; updateMode?: 'full' | 'minimal' | string };

const TEXT = /^(text\/|application\/(json|javascript|xml|x-www-form-urlencoded)|image\/svg)|\+json|\+xml/;
const headerList = (h: Record<string, string>) => Object.entries(h).map(([name, value]) => ({ name, value: String(value) }));

export class HarRecorder {
  private entries: Promise<object | null>[] = [];
  private pages = new WeakSet<Page>();

  constructor(
    private path: string,
    private options: Options,
    private baseURL: string | undefined,
    private only?: Page,
  ) {}

  covers(page: Page): boolean {
    return !this.only || this.only === page;
  }

  attach(page: Page): void {
    if (this.pages.has(page)) return;
    this.pages.add(page);
    const started = new Map<Request, number>();
    page.on('request', (req: Request) => started.set(req, Date.now()));
    page.on('requestfinished', (req: Request) => {
      // The body is read now: it is gone once the page closes.
      this.entries.push(this.entry(req, started.get(req) ?? Date.now()));
    });
  }

  private async entry(req: Request, start: number): Promise<object | null> {
    if (this.options.url !== undefined && !(await urlMatches(this.baseURL, req.url(), this.options.url))) return null;
    const res = await req.response();
    if (!res) return null;
    const mimeType = res.headers()['content-type'] ?? 'x-unknown';
    let content: Record<string, unknown> = { size: -1, mimeType };
    if (this.options.updateContent !== 'omit') {
      const body = await res.body().catch(() => null);
      if (body) {
        content.size = body.length;
        // Playwright's default for a .har file: bodies beside it, by hash.
        if ((this.options.updateContent ?? 'attach') === 'attach') {
          const name = `${createHash('sha1').update(body).digest('hex')}.${mimeType.split(/[/;+]/)[1] ?? 'dat'}`;
          mkdirSync(dirname(this.path), { recursive: true });
          writeFileSync(join(dirname(this.path), name), body);
          content = { ...content, _file: name };
        } else if (TEXT.test(mimeType)) {
          content.text = body.toString('utf8');
        } else {
          content = { ...content, text: body.toString('base64'), encoding: 'base64' };
        }
      }
    }
    const url = new URL(req.url());
    const minimal = this.options.updateMode === 'minimal';
    const postData = req.postData();
    return {
      startedDateTime: new Date(start).toISOString(),
      time: Math.max(0, Date.now() - start),
      request: {
        method: req.method(),
        url: req.url(),
        httpVersion: 'HTTP/1.1',
        cookies: [],
        headers: minimal ? [] : headerList(req.headers()),
        queryString: [...url.searchParams].map(([name, value]) => ({ name, value })),
        ...(postData === null ? {} : { postData: { mimeType: req.headers()['content-type'] ?? '', text: postData } }),
        headersSize: -1,
        bodySize: postData === null ? 0 : Buffer.byteLength(postData),
      },
      response: {
        status: res.status(),
        statusText: res.statusText(),
        httpVersion: 'HTTP/1.1',
        cookies: [],
        headers: headerList(res.headers()),
        content,
        redirectURL: res.headers()['location'] ?? '',
        headersSize: -1,
        bodySize: typeof content.size === 'number' ? content.size : -1,
      },
      cache: {},
      timings: { send: -1, wait: -1, receive: -1 },
    };
  }

  async save(): Promise<void> {
    const entries = (await Promise.all(this.entries)).filter((e): e is object => e !== null);
    mkdirSync(dirname(this.path), { recursive: true });
    const har = { log: { version: '1.2', creator: { name: 'lightning-panda', version: '0' }, pages: [], entries } };
    writeFileSync(this.path, JSON.stringify(har, null, 2));
  }
}
