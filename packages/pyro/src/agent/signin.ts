// Whether a stored identity is still signed in, read from where the app
// lands: `pyro profile login` checks a fresh session this way before saving
// it, and `browser_profiles load` checks a saved one. URLs are printed as
// origin and path only: queries and fragments may carry codes and tokens.
import type { Page } from '../wright/page.js';
import type { LoginInfo } from './profiles.js';

/** Paths of sign-in pages, sign-in link pages and auth callbacks. */
const SIGN_IN = /(^|[/_.-])(log-?in|log-?on|sign-?in|sign_in|auth|oauth2?|sso|saml|verify|magic-?link|otp|passwordless)([/_.-]|$)|(^|\/)sessions?\/new(\/|$)/i;

/** The URL looks like a sign-in page (or an error after one: `?error=`). */
export function looksLikeSignIn(url: string): boolean {
  try {
    const u = new URL(url);
    return SIGN_IN.test(u.pathname) || u.searchParams.has('error');
  } catch {
    return false;
  }
}

/** A URL safe to print: origin and path, without query or fragment, and with long token-like path segments cut. */
export function safeUrl(url: string): string {
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return '(not a URL)';
  }
  if (u.protocol === 'about:') return `about:${u.pathname}`;
  if (!/^https?:$/.test(u.protocol)) return `a ${u.protocol} URL`;
  const path = u.pathname
    .split('/')
    .map((seg) => (seg.length > 24 || (seg.length >= 16 && /\d/.test(seg) && /[a-z]/i.test(seg)) ? '…' : seg))
    .join('/');
  return `${u.origin}${path}`;
}

export type SessionCheck = {
  /** true: signed in; false: the session is gone; undefined: the page does not say. */
  signedIn: boolean | undefined;
  /** Where the app landed (`safeUrl`). */
  landed: string;
  status?: number;
  /** Why, in a few words. */
  why: string;
};

/**
 * Opens `login.url` in `page` and reads whether the session holds: an HTTP
 * 401 or 403, a sign-in page where the signed-in app landed elsewhere, or a
 * missing signed-in element (`login.selector`) mean it does not.
 */
export async function checkSession(page: Page, login: Pick<LoginInfo, 'url' | 'landing' | 'selector'>, timeout: number): Promise<SessionCheck> {
  const response = await page.goto(login.url, { timeout });
  await page.waitForLoadState('networkidle', { timeout: Math.min(timeout, 10_000) }).catch(() => {});
  const status = response?.status();
  const landed = safeUrl(page.url());
  if (status === 401 || status === 403) return { signedIn: false, landed, status, why: `the app answered HTTP ${status} at ${landed}` };
  if (landed !== login.landing && looksLikeSignIn(page.url())) return { signedIn: false, landed, status, why: `it landed on ${landed}, a sign-in page` };
  if (login.selector) {
    const shown = await page
      .locator(login.selector)
      .first()
      .waitFor({ state: 'visible', timeout: 5_000 })
      .then(() => true, () => false);
    if (!shown) return { signedIn: false, landed, status, why: `${landed} does not show ${JSON.stringify(login.selector)}, which it shows when signed in` };
  }
  if (status !== undefined && status >= 400) return { signedIn: undefined, landed, status, why: `the app answered HTTP ${status} at ${landed}` };
  if (landed !== login.landing) return { signedIn: undefined, landed, status, why: `it landed on ${landed}, not on ${login.landing} as when the person signed in` };
  return { signedIn: true, landed, status, why: `it opened ${landed}` };
}
