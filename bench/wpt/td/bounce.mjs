// WebDriver's "Run Bounce Tracking Mitigations" (https://privacycg.github.io/nav-tracking-mitigations/#run-bounce-tracking-mitigations-command):
// runs the browser context's bounce tracking timer with no grace period and returns the sites whose state it deleted,
// carried out by Pyro's `Storage.runBounceTrackingMitigations` (the CDP method Chrome's ChromeDriver uses). The browser
// records the sites a navigation only bounced through (server redirects, client redirects without user activation) and
// deletes the cookies and storage of those the user did not interact with.

export const commands = {
  'run_bounce_tracking_mitigations': async (ctx, [context]) => {
    const page = context ? await ctx.windowPage(context) : ctx.page;
    if (!page) throw new Error('no such window');
    const { deletedSites } = await (await ctx.sessionOf(page)).send('Storage.runBounceTrackingMitigations');
    return deletedSites;
  },
};
