// WebDriver's Get All Cookies and Get Named Cookie (https://w3c.github.io/webdriver/#cookies), for the cookies the
// current document's URL can see (the cookie jar's own matching: domain, path, and Secure against the URL's scheme,
// HttpOnly cookies included), read with Storage.getCookies through Playwright's context.cookies.

// A cookie as WebDriver reports it: `expiry` in seconds, left out for a session cookie.
const webdriver = (c) => ({
  name: c.name, value: c.value, path: c.path, domain: c.domain, secure: c.secure, httpOnly: c.httpOnly, sameSite: c.sameSite,
  ...(c.expires > 0 ? { expiry: Math.floor(c.expires) } : {}),
});

async function visible(ctx) {
  return (await ctx.context.cookies([ctx.page.url()])).map(webdriver);
}

export const commands = {
  'get_all_cookies': async (ctx) => visible(ctx),
  // `null`: no such cookie (testdriver.js rejects with "no such cookie"). The first match of the jar's order, as
  // WebDriver takes the first in the cookies' list.
  'get_named_cookie': async (ctx, [name]) => (await visible(ctx)).find((c) => c.name === name) ?? null,
};
