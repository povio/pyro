import { defineConfig } from 'vitepress';

const guide = [
  {
    text: 'Getting started',
    items: [
      { text: 'What is Pyro?', link: '/guide/introduction' },
      { text: 'Installation', link: '/guide/installation' },
      { text: 'Quick start', link: '/guide/quick-start' },
      { text: 'Choosing an API', link: '/guide/choosing-an-api' },
    ],
  },
  {
    text: 'Concepts',
    items: [
      { text: 'How it works', link: '/guide/how-it-works' },
      { text: 'Settling and auto-waiting', link: '/guide/settling' },
      { text: 'Profiles and pre-booted pages', link: '/guide/profiles' },
      { text: 'Connections and fork mode', link: '/guide/fork-mode' },
      { text: 'Rendering: screenshots and PDF', link: '/guide/rendering' },
    ],
  },
  {
    text: 'More',
    items: [
      { text: 'Platforms and limits', link: '/guide/platforms' },
      { text: 'Performance', link: '/guide/performance' },
      { text: 'Troubleshooting', link: '/guide/troubleshooting' },
      { text: 'FAQ', link: '/guide/faq' },
    ],
  },
];

const playwright = [
  {
    text: 'Coming from Playwright',
    items: [
      { text: 'Overview', link: '/playwright/overview' },
      { text: 'Migrating a suite', link: '/playwright/migration' },
      { text: 'Config and the test runner', link: '/playwright/runner' },
      { text: 'Fixtures, hooks and steps', link: '/playwright/fixtures' },
      { text: 'Assertions (expect)', link: '/playwright/assertions' },
      { text: 'Locators and selectors', link: '/playwright/locators' },
      { text: 'Network, routes and HAR', link: '/playwright/network' },
      { text: 'Screenshots and snapshots', link: '/playwright/snapshots' },
      { text: 'Clock, coverage, ARIA', link: '/playwright/advanced' },
      { text: 'Booting pages ahead', link: '/playwright/boot-pools' },
      { text: 'Behaviour differences', link: '/playwright/differences' },
      { text: 'API coverage', link: '/playwright/api-coverage' },
    ],
  },
];

const e2e = [
  {
    text: 'Fastest E2E tests',
    items: [
      { text: 'Why a native driver', link: '/e2e/overview' },
      { text: 'Your first test', link: '/e2e/first-test' },
      { text: 'Locators', link: '/e2e/locators' },
      { text: 'Steps and checks', link: '/e2e/steps' },
      { text: 'Requests and mocks', link: '/e2e/network' },
      { text: 'Profiles and page pools', link: '/e2e/profiles' },
      { text: 'Fork mode in tests', link: '/e2e/fork' },
      { text: 'Suite runner: lightningpanda e2e', link: '/e2e/runner' },
      { text: 'One server, many workers', link: '/e2e/shared-server' },
      { text: 'Serving a build: --static', link: '/e2e/static' },
      { text: 'Visual tests', link: '/e2e/screenshots' },
      { text: 'Porting from Playwright', link: '/e2e/porting' },
      { text: 'Debugging failures', link: '/e2e/debugging' },
      { text: 'CI recipes', link: '/e2e/ci' },
      { text: 'Writing tests with AI agents', link: '/e2e/ai-agents' },
    ],
  },
];

const scraping = [
  {
    text: 'Scraping and automation',
    items: [
      { text: 'Overview', link: '/scraping/overview' },
      { text: 'One-off fetches (CLI)', link: '/scraping/fetch' },
      { text: 'Scripting a browser', link: '/scraping/library' },
      { text: 'Stored profiles', link: '/scraping/profiles' },
      { text: 'Profiles booted once (fork)', link: '/scraping/fork-profiles' },
      { text: 'Proxies, user agents, headers', link: '/scraping/identity' },
      { text: 'Extracting data', link: '/scraping/extracting' },
      { text: 'Concurrency at scale', link: '/scraping/concurrency' },
      { text: 'Screenshots and PDFs', link: '/scraping/pdf' },
      { text: 'Puppeteer and raw CDP', link: '/scraping/cdp' },
      { text: 'Running as a service', link: '/scraping/service' },
    ],
  },
];

const reference = [
  {
    text: 'Reference',
    items: [
      { text: 'CLI', link: '/reference/cli' },
      { text: 'serve options', link: '/reference/serve' },
      { text: 'Environment variables', link: '/reference/env' },
      { text: 'SDK: Lightningpanda class and Page', link: '/reference/sdk' },
      { text: 'SDK: locators and steps', link: '/reference/steps' },
      { text: 'LP.run wire format', link: '/reference/lp-run' },
      { text: 'Lightningpanda CDP domain', link: '/reference/cdp-domain' },
      { text: 'CDP coverage', link: '/reference/cdp' },
      { text: 'Web platform support', link: '/reference/web-platform' },
      { text: 'Changelog and versions', link: '/reference/versions' },
    ],
  },
];

export default defineConfig({
  title: 'Pyro',
  description:
    'A headless browser on JavaScriptCore for fast E2E tests, Playwright suites and scraping.',
  lang: 'en-US',
  cleanUrls: false,
  lastUpdated: false,
  outDir: '../dist',
  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/logo.svg' }],
    ['meta', { name: 'theme-color', content: '#dc1f26' }],
  ],
  markdown: { lineNumbers: false },
  themeConfig: {
    logo: '/logo.svg',
    nav: [
      { text: 'Guide', link: '/guide/introduction', activeMatch: '^/guide/' },
      { text: 'From Playwright', link: '/playwright/overview', activeMatch: '^/playwright/' },
      { text: 'Fast E2E', link: '/e2e/overview', activeMatch: '^/e2e/' },
      { text: 'Scraping', link: '/scraping/overview', activeMatch: '^/scraping/' },
      { text: 'Reference', link: '/reference/cli', activeMatch: '^/reference/' },
      { text: 'npm', link: 'https://www.npmjs.com/package/@povio/lightning-panda' },
    ],
    sidebar: {
      '/guide/': guide,
      '/playwright/': playwright,
      '/e2e/': e2e,
      '/scraping/': scraping,
      '/reference/': reference,
    },
    search: { provider: 'local' },
    outline: { level: [2, 3] },
    socialLinks: [{ icon: 'npm', link: 'https://www.npmjs.com/package/@povio/lightning-panda' }],
    footer: {
      message: 'AGPL-3.0-only. A derivative of Lightpanda. Built by Povio.',
      copyright: '© 2026 Povio',
    },
  },
});
