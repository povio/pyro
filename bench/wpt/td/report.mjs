// WebDriver's Generate Test Report (https://w3c.github.io/reporting/#generate-test-report-command): the browser queues
// a report of type "test" for the page's document through CDP's Page.generateTestReport, as chromedriver does.

export const commands = {
  'generate_test_report': async (ctx, [message]) => {
    await (await ctx.session()).send('Page.generateTestReport', { message: String(message) });
  },
};
