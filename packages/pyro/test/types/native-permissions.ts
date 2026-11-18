import type { Page } from '../../src/index.js';

declare const page: Page;
const granted: Promise<void> = page.grantPermissions(['clipboardSanitizedWrite'] as const, { origin: 'https://app.example' });
const reset: Promise<void> = page.resetPermissions();
page.grantPermissions([]);
// @ts-expect-error permissions are CDP permission names, not numeric ids
page.grantPermissions([1]);
// @ts-expect-error an origin is a string
page.grantPermissions([], { origin: 1 });
