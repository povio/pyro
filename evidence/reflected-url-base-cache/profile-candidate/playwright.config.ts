import base from '/private/tmp/lp-tos-f9f-batch16-fx/apps/fe/playwright.e2e.config';
process.env.E2E_BROWSER_ENGINE='lightpanda';
for(const k of ['E2E_DOM_ACTIVATION','E2E_FRESH_AUTH_CONTEXT','E2E_ROUTER_SETUP'])process.env[k]='1';
export default {...base,workers:1,retries:0,timeout:300000,projects:[{name:'diagnostic-profile',testDir:'/private/tmp/lp-url-nav-cache-84ab914',testMatch:'profile-nav.e2e.ts'}],reporter:[['list']],outputDir:'/private/tmp/lp-url-nav-cache-84ab914/test-results',use:{...base.use,trace:'off',video:'off',screenshot:'off'}};
