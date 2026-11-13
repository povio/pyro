// Which origins the agent's browser may request: `--allowed-origins` and
// `--blocked-origins`. The session routes every request of its contexts
// through `check`, aborts what it refuses and reports it.
//
// An entry is an origin (`https://app.example.com`, `http://localhost:3000`),
// a host for any scheme and port (`example.com`, `localhost`), or `*.` and a
// domain for its subdomains (`*.example.com`, not `example.com` itself).
// Blocked entries win over allowed ones; with allowed entries, everything
// else is refused. URLs that do not go to the network (`about:`, `data:`,
// `blob:`, `javascript:`) are never refused.

export class OriginPolicy {
  private readonly allowed: Matcher[];
  private readonly blocked: Matcher[];

  constructor(allowed: string[] = [], blocked: string[] = []) {
    this.allowed = allowed.map(matcher);
    this.blocked = blocked.map(matcher);
  }

  /** True when a list is set (requests then go through routing). */
  get active(): boolean {
    return this.allowed.length > 0 || this.blocked.length > 0;
  }

  /** Why `url` may not be requested (a short reason), or null when it may. */
  check(url: string): string | null {
    let u: URL;
    try {
      u = new URL(url);
    } catch {
      return null;
    }
    if (['about:', 'data:', 'blob:', 'javascript:'].includes(u.protocol)) return null;
    if (this.blocked.some((m) => m(u))) return 'blocked origin';
    if (this.allowed.length && !this.allowed.some((m) => m(u))) return 'origin not in the allowed list';
    return null;
  }
}

type Matcher = (u: URL) => boolean;

/** Splits a `--allowed-origins` value: commas, semicolons or whitespace. */
export function originList(value: string | undefined): string[] {
  return (value ?? '').split(/[,;\s]+/).map((s) => s.trim()).filter(Boolean);
}

function matcher(entry: string): Matcher {
  const e = entry.trim().toLowerCase();
  if (e.startsWith('*.')) {
    const domain = e.slice(2);
    return (u) => u.hostname.endsWith(`.${domain}`);
  }
  if (/^[a-z][a-z0-9+.-]*:\/\//.test(e)) {
    let origin: string;
    try {
      origin = new URL(e).origin;
    } catch {
      throw new Error(`not an origin: ${entry}`);
    }
    return (u) => u.origin === origin;
  }
  const host = e.replace(/\/.*$/, '');
  return (u) => u.hostname === host || u.host === host;
}
