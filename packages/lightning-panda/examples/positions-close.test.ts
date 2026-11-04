// TOS apps/fe/e2e/logistics/positions-close.e2e.ts on lightning-panda, for
// `bun test` in apps/fe/e2e (not run in this repository). Same checks as the
// Playwright original: the rendered number, Settings, Close Position, the
// confirmation dialog, the POST and the details refresh, the closed banner and
// the Reopen button; then the reopen flow. Each test seeds through the API and
// makes one browser call: the page comes signed in from a pool and its route
// change is the first step of the run.
import { afterAll, beforeAll, test } from 'bun:test';
import { Lightningpanda, button, check, dialog, text } from '@povio/lightning-panda';

// buildLocalStorageEntries and getEmployeeAccount are module-private in
// auth.runtime.ts today; the port exports them.
import { buildLocalStorageEntries, getAuthTokens, getEmployeeAccount } from '../fixtures/auth.runtime';
import { e2eEnv } from '../fixtures/env';
import { feE2eSeed } from '../fixtures/seed';

const OFFICE_ID = feE2eSeed.core.office.id;
const positionsApi = `/offices/${OFFICE_ID}/positions`;
const overview = (id: string) => `/platform/logistics/positions/${id}/overview`;

let lp: Lightningpanda;
let token: string;

beforeAll(async () => {
  const tokens = await getAuthTokens();
  token = tokens.accessToken;
  lp = await Lightningpanda.connect(process.env.LIGHTNINGPANDA_WS ?? 'ws://127.0.0.1:9222/');
  await lp.profile('employee', {
    url: `${e2eEnv.feUrl}/platform`,
    // The entries createFreshAuthenticatedContext writes: tokens, user and routing stores.
    localStorage: Object.fromEntries(
      buildLocalStorageEntries(tokens, await getEmployeeAccount()).map((e) => [e.name, e.value]),
    ),
    viewport: { width: 1440, height: 900 },
    pool: 4,
  });
});

afterAll(() => lp?.close());

const api = async (method: string, path: string, body?: unknown) => {
  const r = await fetch(`${e2eEnv.apiUrl}${path}`, {
    method,
    headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${await r.text()}`);
  return r.json();
};

const createPosition = (): Promise<{ id: string; number: string }> =>
  api('POST', positionsApi, {
    section: 'Logistics',
    direction: 'Export',
    transportMode: 'Sea',
    loadType: 'FCL',
    serviceType: 'PortToPort',
    estimatedServiceDate: '2026-12-15T00:00:00.000Z',
    customerBusinessPartnerId: feE2eSeed.businessEntities.businessPartner.list.id,
  });

test.concurrent('closes an open Position from the Position overview', async () => {
  const position = await createPosition();
  await using page = await lp.page({ as: 'employee', route: overview(position.id) });

  await page.run([
    check(text(position.number)).visible(),
    button('Settings', { exact: true }).press('Enter'),
    button('Close Position').visible().first().click(),
    // Settles after the POST and the details refresh it triggers.
    dialog().hasText('This position will be closed').last().button('Close Position', { exact: true }).click(),
    check.request('POST', `${positionsApi}/${position.id}/close`).status(200),
    check.request('GET', `${positionsApi}/${position.id}`).status(200),
    check(text(/^Position Closed · .* · Manual · Closed by:/).visible()).count(1),
    check(button('Reopen Position')).count(1),
  ]);
});

test.concurrent('reopens the same closed Position from the closed banner', async () => {
  const position = await createPosition();
  await api('POST', `${positionsApi}/${position.id}/close`);
  await using page = await lp.page({ as: 'employee', route: overview(position.id) });

  const reopen = dialog().hasText('This position will be reopened').last();
  await page.run([
    button('Reopen Position').click(),
    check(reopen.text('Keep open for days')).visible(),
    reopen.button('Reopen Position', { exact: true }).click(),
    check.request('POST', `${positionsApi}/${position.id}/open`).status(200),
    check(text(`Position ${position.number} has been reopened.`).first()).visible(),
    check(text('Position Closed', { exact: true })).count(0),
    check(button('Reopen Position')).count(0),
    button('Settings', { exact: true }).press('Enter'),
    check(button('Close Position').visible()).count(1),
  ]);
});
