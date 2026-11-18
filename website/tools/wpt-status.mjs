// Generates the WPT status pages (docs/status/) and their data (docs/public/status-data/) before the build, from
// the wpt-results branch; with --pages, after the build, one HTML file per directory (bench/wpt/status-export.mjs). WPT_RESULTS_DIR points at a checkout of that branch;
// without it the branch is fetched from origin. With no results the page says so and the build goes on.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { copyPages, writeSite } from '../../bench/wpt/status-export.mjs';

const site = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
if (process.argv.includes('--pages')) {
  console.log(`wpt-status: ${copyPages(path.join(site, 'dist'))} directory pages`);
  process.exit(0);
}
let data = process.env.WPT_RESULTS_DIR || null;
let tmp = null;
if (!data) {
  try {
    const git = (...a) => execFileSync('git', a, { cwd: path.dirname(site), stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 1 << 30 });
    git('fetch', '--quiet', '--depth=1', 'origin', 'wpt-results');
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wpt-results-'));
    // From the repository's root: run in a subdirectory, git archive takes only that subdirectory.
    execFileSync('tar', ['-x', '-C', tmp], { input: git('archive', 'FETCH_HEAD') });
    data = tmp;
  } catch (e) {
    console.warn(`wpt-status: no wpt-results branch (${String(e.message).split('\n')[0]}); placeholder page only`);
  }
}
const n = writeSite(data, path.join(site, 'docs'));
console.log(`wpt-status: data for ${n} directories`);
if (tmp) fs.rmSync(tmp, { recursive: true, force: true });
