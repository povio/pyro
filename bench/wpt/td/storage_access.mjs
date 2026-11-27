// WebDriver's "Set Storage Access" (https://privacycg.github.io/storage-access/#set-storage-access-command): a rule of the
// browser context that blocks or allows the unpartitioned cookies of `origin` embedded in a third-party context under
// `embedding_origin` (`*` for any), carried out by Pyro's `Browser.setStorageAccess` (Chrome has no CDP method for it; its
// ChromeDriver sets the content setting directly). Without a rule third-party cookies are available, as in Chrome.

export const commands = {
  'set_storage_access': async (ctx, [origin, embeddingOrigin, blocked, context]) => {
    if (typeof origin !== 'string' || typeof embeddingOrigin !== 'string') throw new Error('invalid argument: origin and embedding_origin must be strings');
    if (typeof blocked !== 'boolean') throw new Error('invalid argument: blocked must be a boolean');
    const page = context ? await ctx.windowPage(context) : ctx.page;
    if (!page) throw new Error('no such window');
    await (await ctx.sessionOf(page)).send('Browser.setStorageAccess', { origin, embeddingOrigin, blocked });
  },
};
