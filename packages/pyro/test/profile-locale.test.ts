// A profile's `locale` and `timezoneId`, as Playwright's context options:
// navigator.language, Accept-Language, Intl's default locale, the time zone.
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { join } from 'node:path';
import { Pyro, evaluate } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');

let site: ReturnType<typeof Bun.serve>;
const languages: string[] = [];
beforeAll(() => {
  site = Bun.serve({
    port: 0,
    fetch(req) {
      languages.push(req.headers.get('accept-language') ?? '');
      return new Response('<!doctype html><p>app</p>', { headers: { 'content-type': 'text/html' } });
    },
  });
});
afterAll(() => site.stop(true));
const url = () => `http://127.0.0.1:${site.port}/app`;

const probe = [
  evaluate('navigator.language', 'language'),
  evaluate('new Intl.NumberFormat().format(1234.5)', 'number'),
  evaluate('new Date(Date.UTC(2026, 0, 15, 12)).toLocaleDateString()', 'date'),
  evaluate('Intl.DateTimeFormat().resolvedOptions().timeZone', 'zone'),
  evaluate('new Date(Date.UTC(2026, 0, 15, 12)).getHours()', 'hours'),
];

for (const fork of [false, true]) {
  test(`profile locale and timezoneId (${fork ? 'fork mode' : 'one connection'})`, async () => {
    const lp = await Pyro.launch({ bin, args: fork ? ['--fork'] : [] });
    try {
      languages.length = 0;
      await lp.profile('swiss', { url: url(), locale: 'de-DE', timezoneId: 'Asia/Tokyo' });
      await using page = await lp.page({ as: 'swiss' });
      const r = await page.run(probe);
      expect(r.values).toEqual({ language: 'de-DE', number: '1.234,5', date: '15.1.2026', zone: 'Asia/Tokyo', hours: 21 });
      expect(languages.every((l) => l.startsWith('de-DE'))).toBe(true);
      // A fresh page of the profile (its own init scripts) gets them too.
      await using fresh = await lp.page({ as: 'swiss', initScripts: ['window.fresh = 1'] });
      const f = await fresh.run([...probe, evaluate('window.fresh', 'fresh')]);
      expect(f.values).toEqual({ language: 'de-DE', number: '1.234,5', date: '15.1.2026', zone: 'Asia/Tokyo', hours: 21, fresh: 1 });
      // The zone is the VM's: another profile with another zone is refused.
      await expect(lp.profile('elsewhere', { url: url(), timezoneId: 'America/New_York' })).rejects.toThrow('another profile of this server has Asia/Tokyo');
      await expect(lp.profile('bad', { url: url(), locale: 'de CH' })).rejects.toThrow('locale must be a BCP 47 language tag');
    } finally {
      lp.close();
    }
  }, 30000);
}
