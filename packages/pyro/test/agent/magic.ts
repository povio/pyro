// A sign-in-by-email app for the stored identity tests. `link()` issues a
// single-use sign-in link; opening it sets an httpOnly session cookie and a
// localStorage token, then redirects to the app, which is signed in only
// with a live session cookie (else it redirects to /login). A used or unknown
// link redirects to /login?error=link-expired.

export type MagicSite = {
  url: string;
  /** A fresh single-use sign-in link. */
  link(): string;
  /** Session cookie values and localStorage tokens handed out: secrets that must never be printed. */
  secrets: string[];
  /** Ends every session (the next visit lands on the sign-in page). */
  revokeAll(): void;
  stop(): void;
};

const page = (title: string, body: string) =>
  new Response(`<!doctype html><html><head><title>${title}</title></head><body>${body}</body></html>`, { headers: { 'content-type': 'text/html' } });

export function startMagicSite(): MagicSite {
  const tokens = new Set<string>();
  const sessions = new Set<string>();
  const secrets: string[] = [];
  const server = Bun.serve({
    port: 0,
    fetch(req) {
      const url = new URL(req.url);
      const sid = /(?:^|;\s*)sid=([^;]+)/.exec(req.headers.get('cookie') ?? '')?.[1];
      switch (url.pathname) {
        case '/magic': {
          const token = url.searchParams.get('token') ?? '';
          if (!tokens.delete(token)) return new Response(null, { status: 302, headers: { location: '/login?error=link-expired' } });
          const id = crypto.randomUUID();
          const local = `lt-${crypto.randomUUID()}`;
          sessions.add(id);
          secrets.push(id, local);
          return new Response(
            `<!doctype html><title>Signing in</title><p>Signing you in…</p><script>localStorage.setItem('authToken', ${JSON.stringify(local)}); setTimeout(() => location.replace('/app'), 50);</script>`,
            { headers: { 'content-type': 'text/html', 'set-cookie': `sid=${id}; Path=/; HttpOnly; Max-Age=3600; SameSite=Lax` } },
          );
        }
        case '/app':
          if (!sid || !sessions.has(sid)) return new Response(null, { status: 302, headers: { location: '/login' } });
          return page(
            'Dashboard',
            `<nav id="account">Signed in as alice</nav><h1>Dashboard</h1><p id="ls"></p>
             <script>document.getElementById('ls').textContent = localStorage.getItem('authToken') ? 'token kept' : 'no token';</script>`,
          );
        case '/login':
          return page('Sign in', `<h1>Sign in</h1><form><label>Email <input name="email"></label><button>Email me a link</button></form>`);
        default:
          return new Response('not found', { status: 404 });
      }
    },
  });
  const base = `http://127.0.0.1:${server.port}`;
  return {
    url: base,
    link() {
      const token = `tok${crypto.randomUUID().replace(/-/g, '')}`;
      tokens.add(token);
      secrets.push(token);
      return `${base}/magic?token=${token}`;
    },
    secrets,
    revokeAll: () => sessions.clear(),
    stop: () => server.stop(true),
  };
}
