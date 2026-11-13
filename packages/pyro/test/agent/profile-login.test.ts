// Stored identities signed in by a person: `pyro profile login` against a
// local sign-in-by-email app (the link on stdin, or typed at a terminal
// without echo), the saved file's modes, output without secrets, a used link,
// then `browser_profiles load` and `pyro mcp --profile` in fresh MCP sessions
// signed in, and the hint when the session has expired.
import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startMagicSite, type MagicSite } from './magic';
import { pkg, start, text } from './mcp-client';
import { bin } from './site';

const cli = join(pkg, 'src/cli.ts');
const python = spawnSync('python3', ['--version']).status === 0 ? 'python3' : undefined;

let site: MagicSite;
let dir: string;
let profiles: string;

beforeAll(() => {
  site = startMagicSite();
  dir = mkdtempSync(join(tmpdir(), 'pyro-login-'));
  profiles = join(dir, 'profiles');
});
afterAll(() => {
  site?.stop();
  rmSync(dir, { recursive: true, force: true });
});

/** `pyro profile <args>`, `stdin` written to it; async, so this process's test site keeps serving. */
async function pyro(args: string[], stdin = ''): Promise<{ code: number; out: string; err: string }> {
  const p = Bun.spawn(['bun', cli, 'profile', ...args], { cwd: pkg, stdin: new TextEncoder().encode(stdin), stdout: 'pipe', stderr: 'pipe', env: { ...process.env, PYRO_BIN: bin } });
  const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited]);
  return { code, out, err };
}
const login = (link: string, extra: string[] = []) => pyro(['login', 'sandbox', '--url', `${site.url}/app`, '--profile-dir', profiles, ...extra], `${link}\n`);
/** Secrets of the site (session cookies, localStorage tokens, link tokens) found in `text`. */
const leaked = (text: string) => site.secrets.filter((s) => text.includes(s));

describe.skipIf(!bin)('pyro profile login', () => {
  test('signs in with a link on stdin and saves the session owner-only, printing no values', async () => {
    const link = site.link();
    const r = await login(link);
    expect(r.code).toBe(0);
    expect(r.out).toContain(`Saved stored identity "sandbox" to ${join(profiles, 'sandbox.json')} (mode 600, in a mode 700 directory).`);
    expect(r.out).toContain(`the link led to ${site.url}/app`);
    expect(r.out).toContain(`a fresh browser context with the saved session opened ${site.url}/app, not a sign-in page`);
    expect(r.out).toContain('cookies:       1 (127.0.0.1: 1)');
    expect(r.out).toMatch(/session:       session cookie: sid \(httpOnly\) for 127\.0\.0\.1 expires \d{4}-\d\d-\d\d \d\d:\d\d UTC \(in (59|60) min\)/);
    expect(r.out).toContain(`localStorage:  ${site.url} (1 key)`);
    expect(statSync(join(profiles, 'sandbox.json')).mode & 0o777).toBe(0o600);
    expect(statSync(profiles).mode & 0o777).toBe(0o700);
    // No cookie value, storage value or link anywhere in what was printed.
    expect(leaked(r.out + r.err)).toEqual([]);
    expect(r.out + r.err).not.toContain('token=');
    // The file holds the session: the httpOnly cookie, the localStorage token, and where to check it.
    const saved = JSON.parse(readFileSync(join(profiles, 'sandbox.json'), 'utf8'));
    expect(saved.cookies).toEqual([expect.objectContaining({ name: 'sid', httpOnly: true, domain: '127.0.0.1' })]);
    expect(site.secrets).toContain(saved.cookies[0].value);
    expect(saved.origins).toEqual([{ origin: site.url, localStorage: [{ name: 'authToken', value: expect.stringMatching(/^lt-/) }] }]);
    expect(saved.pyro.login).toMatchObject({ url: `${site.url}/app`, landing: `${site.url}/app` });
    expect(JSON.stringify(saved)).not.toContain(link);
    // list and show: origins, counts, expiries; never values.
    const listed = await pyro(['list', '--profile-dir', profiles]);
    expect(listed.out).toContain(`sandbox: saved`);
    expect(listed.out).toContain(`signed in by a person at ${site.url}/app`);
    const shown = await pyro(['show', 'sandbox', '--profile-dir', profiles]);
    expect(shown.out).toContain('state file:    sandbox.json (mode 600)');
    expect(shown.out).toContain(`in ${profiles} (mode 700)`);
    const json = await pyro(['show', 'sandbox', '--profile-dir', profiles, '--json']);
    expect(JSON.parse(json.out)).toMatchObject({ name: 'sandbox', cookies: 1, localStorage: [{ origin: site.url, keys: 1 }], modes: { dir: '700', state: '600' } });
    expect(leaked(listed.out + shown.out + json.out)).toEqual([]);
  }, 60_000);

  test('a used link exits 1 and says why, without printing it', async () => {
    const link = site.link();
    expect((await login(link, ['--profile-dir', join(dir, 'other')])).code).toBe(0);
    const before = readFileSync(join(profiles, 'sandbox.json'), 'utf8');
    const r = await login(link);
    expect(r.code).toBe(1);
    expect(r.err).toContain('the sign-in did not complete: the link led to a sign-in or error page; the page is at');
    expect(r.err).toContain('/login (title "Sign in")');
    expect(r.err).toContain('The link may have expired or been used already');
    expect(leaked(r.out + r.err)).toEqual([]);
    // The identity saved before is left as it was.
    expect(readFileSync(join(profiles, 'sandbox.json'), 'utf8')).toBe(before);
    // Usage mistakes exit 2.
    expect((await pyro(['login', 'sandbox', '--profile-dir', profiles], `${site.link()}\n`)).code).toBe(2);
    expect((await pyro(['login', 'sandbox', '--url', `${site.url}/app`, '--profile-dir', profiles], 'not a link\n')).err).toContain('not an http(s) link');
  }, 60_000);

  test.skipIf(!python)('reads the link at a terminal without echoing it', async () => {
    const link = site.link();
    // A pseudo-terminal: the link is typed once the prompt shows, with a typo fixed by backspace.
    const drive = `
import os, pty, select, sys, time
link, cmd = sys.argv[1], sys.argv[2:]
pid, fd = pty.fork()
if pid == 0:
    os.execvp(cmd[0], cmd)
out = b''
def pump(until, timeout):
    global out
    end = time.time() + timeout
    while time.time() < end:
        if not select.select([fd], [], [], 0.1)[0]:
            continue
        try:
            data = os.read(fd, 4096)
        except OSError:
            return
        if not data:
            return
        out += data
        if until():
            return
pump(lambda: b'Paste the sign-in link' in out, 30)
os.write(fd, (link + 'X\\x7f\\r').encode())
pump(lambda: False, 90)
_, status = os.waitpid(pid, 0)
sys.stdout.write(out.decode(errors='replace'))
sys.stdout.write('\\nEXIT %d' % os.waitstatus_to_exitcode(status))
`;
    const p = Bun.spawn([python!, '-I', '-c', drive, link, 'bun', cli, 'profile', 'login', 'typed', '--url', `${site.url}/app`, '--profile-dir', profiles], {
      cwd: pkg,
      stdout: 'pipe',
      stderr: 'pipe',
      env: { ...process.env, PYRO_BIN: bin },
    });
    const [out] = await Promise.all([new Response(p.stdout).text(), p.exited]);
    expect(out).toContain('Paste the sign-in link from the email (it is not shown)');
    expect(out).toContain('Saved stored identity "typed"');
    expect(out).toContain('EXIT 0');
    expect(out).not.toContain(link.slice(-24));
    expect(leaked(out)).toEqual([]);
  }, 120_000);
});

describe.skipIf(!bin)('loading an identity a person signed in', () => {
  test('browser_profiles load in a fresh MCP session is signed in; the model sees no cookie values', async () => {
    const c = start(['bun', 'src/cli.ts'], ['--profile-dir', profiles, '--output-dir', join(dir, 'out')]);
    try {
      await c.request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '1' } });
      const r = await c.call('browser_profiles', { action: 'load', name: 'sandbox' });
      expect(r.result.isError).toBeUndefined();
      const t = text(r);
      expect(t).toContain('Loaded profile "sandbox" into a new tab [0] with a context of its own: 1 cookie, localStorage of');
      expect(t).toContain(`Opened ${site.url}/app: signed in (it opened ${site.url}/app, as when the person signed in`);
      expect(t).toContain('Signed in as alice');
      expect(t).toContain('token kept');
      const listed = await c.call('browser_profiles', { action: 'list' });
      expect(text(listed)).toContain(`- sandbox: saved`);
      expect(leaked(c.raw)).toEqual([]);
    } finally {
      await c.end();
    }
  }, 60_000);

  test('pyro mcp --profile starts signed in', async () => {
    const c = start(['bun', 'src/cli.ts'], ['--profile-dir', profiles, '--profile', 'sandbox', '--output-dir', join(dir, 'out')]);
    try {
      await c.request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '1' } });
      const r = await c.call('browser_navigate', { url: `${site.url}/app` });
      expect(text(r)).toContain('heading "Dashboard"');
      expect(leaked(c.raw)).toEqual([]);
    } finally {
      await c.end();
    }
  }, 60_000);

  test('an expired session says so and tells the agent to ask the person to sign in again', async () => {
    site.revokeAll();
    const c = start(['bun', 'src/cli.ts'], ['--profile-dir', profiles, '--output-dir', join(dir, 'out')]);
    try {
      await c.request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '1' } });
      const r = await c.call('browser_profiles', { action: 'load', name: 'sandbox' });
      expect(r.result.isError).toBe(true);
      const t = text(r);
      expect(t).toContain(`Opened ${site.url}/app to check the session: it landed on ${site.url}/login, a sign-in page.`);
      expect(t).toContain('The saved session of stored identity "sandbox" has expired');
      expect(t).toContain('Do not sign in yourself');
      expect(t).toContain(`ask the person to run \`pyro profile login sandbox --url ${site.url}/app --profile-dir ${profiles}\``);
      expect(t.match(/pyro profile login/g)).toHaveLength(1);
      expect(leaked(c.raw)).toEqual([]);
    } finally {
      await c.end();
    }
    // Started in the identity (no check on load), the agent is warned when the app shows its sign-in page.
    const started = start(['bun', 'src/cli.ts'], ['--profile-dir', profiles, '--profile', 'sandbox', '--output-dir', join(dir, 'out')]);
    try {
      await started.request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '1' } });
      const t = text(await started.call('browser_navigate', { url: `${site.url}/app` }));
      expect(t).toContain('heading "Sign in"');
      expect(t).toContain('This tab is stored identity "sandbox" and the page looks like a sign-in page: its session may have expired. Do not sign in yourself');
    } finally {
      await started.end();
    }
    // Signed in again by the person, it loads signed in.
    expect((await login(site.link())).code).toBe(0);
    const again = start(['bun', 'src/cli.ts'], ['--profile-dir', profiles, '--output-dir', join(dir, 'out')]);
    try {
      await again.request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '1' } });
      expect(text(await again.call('browser_profiles', { action: 'load', name: 'sandbox' }))).toContain('signed in');
    } finally {
      await again.end();
    }
  }, 120_000);

  test('cookies past their expiry: the load says so before opening anything', async () => {
    const old = join(dir, 'old');
    mkdirSync(old, { recursive: true, mode: 0o700 });
    const expired = Math.floor(Date.now() / 1000) - 3600;
    writeFileSync(join(old, 'stale.json'), JSON.stringify({ cookies: [{ name: 'session', value: 'secret-stale-value', domain: '127.0.0.1', path: '/', expires: expired, httpOnly: true, secure: false, sameSite: 'Lax' }], origins: [] }));
    const c = start(['bun', 'src/cli.ts'], ['--profile-dir', old, '--output-dir', join(dir, 'out')]);
    try {
      await c.request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'test', version: '1' } });
      const r = await c.call('browser_profiles', { action: 'load', name: 'stale' });
      expect(r.result.isError).toBe(true);
      expect(text(r)).toMatch(/has expired: its session cookies expired \d{4}-\d\d-\d\d \d\d:\d\d UTC \(\d+ (min|h) ago\)/);
      expect(text(r)).toContain(`pyro profile login stale --url <the app URL> --profile-dir ${old}`);
      expect(c.raw).not.toContain('secret-stale-value');
    } finally {
      await c.end();
    }
    const listed = await pyro(['list', '--profile-dir', old]);
    expect(listed.out).toContain('EXPIRED: its session cookies expired');
    expect((await pyro(['delete', 'stale', '--profile-dir', old])).out).toContain('Deleted the saved session of "stale".');
  }, 60_000);
});
