// `serve --fork`: a profile whose setup fails is reported to the clients
// that register, adopt or derive from it, and none of its pages is handed out.
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { join } from 'node:path';
import { Pyro, ProfileSetupError, check, css, evaluate } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');

// `/app` shows `#late` only once the test lets it (`fixed`), and `/slow` after 300 ms.
let fixed = false;
let site: ReturnType<typeof Bun.serve>;
beforeAll(() => {
  site = Bun.serve({
    port: 0,
    fetch(req) {
      const path = new URL(req.url).pathname;
      // `/boot`: the app lands (replaceState to `/landing`) once a 2.5 s request answers.
      if (path === '/api/slow') return new Promise((r) => setTimeout(() => r(new Response('{}', { headers: { 'content-type': 'application/json' } })), 2500));
      if (path === '/boot') {
        const app = '<script>fetch("/api/slow").then(() => { history.replaceState(null, "", "/landing"); document.body.dataset.landed = "1"; })</script>';
        return new Response(`<!doctype html><div id="ready">ready</div>${app}`, { headers: { 'content-type': 'text/html' } });
      }
      const late = path === '/slow'
        ? '<script>setTimeout(() => document.body.insertAdjacentHTML("beforeend", "<i id=late>late</i>"), 300)</script>'
        : fixed ? '<i id="late">late</i>' : '';
      return new Response(`<!doctype html><div id="ready">ready</div>${late}`, { headers: { 'content-type': 'text/html' } });
    },
  });
});
afterAll(() => site.stop(true));
const url = (path = '/app') => `http://127.0.0.1:${site.port}${path}`;

const failing = () => ({ url: url(), setup: [check(css('#ready')).visible(), css('#missing').click({ timeout: 300 })] });
const listed = async (lp: Pyro) =>
  (await lp.cdp.send('Pyro.preparedProfiles')).profiles.map((p: { name: string; holders: number; state: string }) => `${p.name}:${p.holders}:${p.state}`);

test('a failing setup step rejects profile() with the step, and its pages, adopters and derived profiles see it', async () => {
  const launcher = await Pyro.launch({ bin, args: ['--fork'] });
  const worker = await Pyro.connect(launcher.wsEndpoint);
  try {
    const error = await launcher.profile('app', failing()).then(() => undefined, (e) => e);
    expect(error).toBeInstanceOf(ProfileSetupError);
    expect(error.profile).toBe('app');
    expect(error.step).toBe(1);
    expect(error.message).toStartWith('profile app: setup step 1 (Click {"css":"#missing"}): timed out after 300 ms: ');
    expect(error.message).toContain('matched no element');
    // No half-set-up page: not from this client, nor from another.
    await expect(launcher.page({ as: 'app' })).rejects.toThrow('profile app: setup step 1 (Click');
    await expect(launcher.warm('app')).rejects.toThrow('setup step 1');
    await expect(worker.adopt('app')).rejects.toBeInstanceOf(ProfileSetupError);
    await expect(worker.page({ as: 'app' })).rejects.toThrow('profile app: setup step 1 (Click');
    // Derived profiles fail with their base: this client's, and another's (`base: name` adopts).
    const derived = { base: 'app', url: url(), setup: [evaluate('window.derived = true')] };
    await expect(launcher.profile('feature', derived)).rejects.toThrow('profile feature: base profile app failed: setup step 1 (Click');
    await expect(worker.profile('other', derived)).rejects.toThrow('profile app: setup step 1 (Click');
    await expect(launcher.page({ as: 'feature' })).rejects.toThrow('base profile app failed');
    // The server keeps the failed profiles for their holders (adopters hold nothing).
    expect(await listed(launcher)).toEqual(['app:1:failed', 'feature:1:failed']);
    // A raw client: the profile's acquisition fails with the error, not "no page" (-32602).
    const { profiles } = await launcher.cdp.send('Pyro.preparedProfiles');
    const id = profiles.find((p: { name: string }) => p.name === 'app').profileId;
    const raw = await Pyro.connect(`${launcher.wsEndpoint}${launcher.wsEndpoint.includes('?') ? '&' : '?'}profile=${id}`);
    try {
      const acquired = await raw.cdp.send('Pyro.acquirePreparedPage', { profileId: id, attach: true }).catch((e) => e);
      expect(acquired.code).toBe(-32000);
      expect(acquired.message).toContain(`profile ${id}: setup step 1 (Click`);
      expect((await raw.cdp.send('Pyro.preparedProfileStatus', { profileId: id })).state).toBe('failed');
    } finally {
      raw.close();
    }
  } finally {
    worker.close();
    launcher.close();
  }
}, 30000);

test('a profile derived from a base whose setup is still running fails when the base does', async () => {
  const lp = await Pyro.launch({ bin, args: ['--fork'] });
  try {
    // The base fails once `#late` shows: profiles derived from it are registered meanwhile.
    const [base, derived, deeper] = await Promise.allSettled([
      lp.profile('slow', { url: url('/slow'), setup: [check(css('#late')).visible(), css('#missing').click({ timeout: 300 })] }),
      lp.profile('child', { base: 'slow', url: url('/slow'), setup: [evaluate('window.child = true')] }),
      lp.profile('grandchild', { base: 'child', url: url('/slow'), setup: [evaluate('window.grandchild = true')] }),
    ]);
    const reasons = [base, derived, deeper].map((r) => (r.status === 'rejected' ? (r.reason as Error).message : 'resolved'));
    expect(reasons[0]).toStartWith('profile slow: setup step 1 (Click');
    expect(reasons[1]).toStartWith('profile child: base profile slow failed: setup step 1 (Click');
    expect(reasons[2]).toStartWith('profile grandchild: base profile child failed: base profile slow failed: setup step 1 (Click');
    await expect(lp.page({ as: 'grandchild' })).rejects.toBeInstanceOf(ProfileSetupError);
    // A setup step that throws: its error, under the step's index.
    await expect(lp.profile('throws', { url: url(), setup: [evaluate('null'), evaluate('Promise.reject(new Error("no sign-in"))')] }))
      .rejects.toThrow(/^profile throws: setup step 1 \(eval\): .*no sign-in/);
  } finally {
    lp.close();
  }
}, 30000);

test('declaring a failed profile again tries again, for every client holding it', async () => {
  fixed = false;
  const launcher = await Pyro.launch({ bin, args: ['--fork'] });
  const worker = await Pyro.connect(launcher.wsEndpoint);
  try {
    const def = { url: url(), setup: [check(css('#late')).visible({ timeout: 300 }), evaluate('window.signedIn = true')] };
    await expect(launcher.profile('app', def)).rejects.toThrow('profile app: setup step 0');
    await expect(worker.profile('app', def)).rejects.toThrow('profile app: setup step 0');
    expect(await listed(launcher)).toEqual(['app:2:failed']);
    // The app is fixed: the same definition boots again, and the name moves to it.
    fixed = true;
    await launcher.profile('app', def);
    expect(await listed(launcher)).toEqual(['app:2:ready']);
    await using page = await launcher.page({ as: 'app' });
    expect((await page.run([evaluate('window.signedIn && !!document.querySelector("#late")', 'ok')])).values.ok).toBe(true);
    // The other holder still has the old failure until it asks again.
    await expect(worker.page({ as: 'app' })).rejects.toThrow('setup step 0');
    await worker.profile('app', def);
    await using again = await worker.page({ as: 'app' });
    expect((await again.run([evaluate('window.signedIn', 'ok')])).values.ok).toBe(true);
    // A changed definition after a failure replaces it too.
    fixed = false;
    await expect(launcher.profile('changed', failing())).rejects.toThrow('setup step 1');
    await launcher.profile('changed', { url: url(), setup: [evaluate('window.changed = 1')] });
    await using changed = await launcher.page({ as: 'changed' });
    expect((await changed.run([evaluate('window.changed', 'v')])).values.v).toBe(1);
  } finally {
    worker.close();
    launcher.close();
  }
}, 30000);

test('a setup runs once the page has landed, also while a connection waits, and forks start from there', async () => {
  const lp = await Pyro.launch({ bin, args: ['--fork'] });
  try {
    await lp.profile('plain', { url: url() });
    const landing = lp.profile('landing', { url: url('/boot'), setup: [evaluate('window.setupAt = location.pathname')] });
    // Registered (booting) before a connection arrives: that connection waits for every page of the template.
    for (let i = 0; !(await listed(lp)).some((p) => p.startsWith('landing:')); i++) {
      if (i > 500) throw new Error('landing was not registered');
      await Bun.sleep(5);
    }
    await using plain = await lp.page({ as: 'plain' });
    expect((await plain.run([evaluate('location.pathname', 'path')])).values.path).toBe('/app');
    await landing;
    await using page = await lp.page({ as: 'landing' });
    const r = await page.run([evaluate('window.setupAt', 'at'), evaluate('location.pathname', 'path'), evaluate('document.body.dataset.landed', 'landed')]);
    expect(r.values).toEqual({ at: '/landing', path: '/landing', landed: '1' });
  } finally {
    lp.close();
  }
}, 30000);
