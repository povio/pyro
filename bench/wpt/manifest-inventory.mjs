// Upstream URLs, globals and variants are authoritative even when the source
// is UTF-16, lives directly in a split directory, or quotes a variant value.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { H2_NAME, NEEDS_H2, TESTHARNESS_SKIP, WPTRUNNER_VIEWPORT } from './lib.mjs';

export function manifestInventory(wpt, manifest, { dirs, all, http2, testdriver, selected, measureReviewedSkips = false }) {
  if (manifest.url_base !== '/' || !manifest.items?.testharness) throw new Error('expected the root WPT testharness manifest');
  const reviewed = measureReviewedSkips ? JSON.parse(fs.readFileSync(new URL('./reviewed-gate-skips.json', import.meta.url), 'utf8')) : {};
  const tests = [], omissions = [], excluded = {};
  const unmatched = new Set(selected);
  const byDir = [...dirs].sort((a, b) => b.length - a.length);
  function omit(record, reason) {
    omissions.push({ ...record, reason });
    excluded[reason] = (excluded[reason] ?? 0) + 1;
  }
  function visit(tree, prefix) {
    for (const [name, entry] of Object.entries(tree)) {
      if (!name || name === '.' || name === '..' || /[\\/]/.test(name)) throw new Error('invalid test source segment');
      const source = prefix + name;
      if (!Array.isArray(entry)) { visit(entry, source + '/'); continue; }
      const dir = byDir.find(d => source.startsWith(d + '/')) ?? (all ? source.split('/')[0] : null);
      if (!dir) continue;
      const [blob, ...items] = entry;
      let verified = false;
      for (const [raw, meta] of items) {
        const url = (raw || source).replace(/^\//, '');
        if (selected && !selected.has(url)) continue;
        unmatched.delete(url);
        const file = url.split(/[?#]/, 1)[0];
        const suffix = file.match(/\.(worker(?:-module)?|sharedworker(?:-module)?|serviceworker(?:-module)?|window-module)\.html$/)?.[1];
        const environment = meta.jsshell ? 'jsshell' : source.startsWith('web-extensions/') ? 'extension' : suffix?.replace(/^worker/, 'dedicatedworker') ?? 'window';
        const record = { dir, url, source, blob, environment };
        const skip = TESTHARNESS_SKIP.find(s => s.path === source || (s.prefix && source.startsWith(s.prefix)));
        if (skip && reviewed[url] !== blob) { omit(record, skip.why); continue; }
        if (skip) record.gateSkipReason = skip.why;
        // The HTTP forward proxy parses and reserializes responses. Byte-level
        // HTTP/1 parsing fixtures need a direct browser-to-origin connection.
        if (source.startsWith('fetch/h1-parsing/')) { omit(record, 'requires direct HTTP/1 origin bytes'); continue; }
        if (meta.jsshell) { omit(record, 'requires JS shell'); continue; }
        if (source.startsWith('web-extensions/')) { omit(record, 'requires browser extension harness'); continue; }
        if (/(?:^|\/)(?:crashtests?|crashes)(?:\/|$)|(?:^|[./_-])(?:crash|mxss|xss|exploit|cve-\d+)(?:[./_-]|$)/i.test(source)) {
          omit(record, 'crash or vulnerability fixture (inventory only)'); continue;
        }
        if (meta.pac) { omit(record, 'requires manifest proxy auto-configuration'); continue; }
        if (meta.testdriver_features?.length) { omit(record, 'unsupported testdriver features: ' + meta.testdriver_features.join(', ')); continue; }
        if (meta.testdriver && !testdriver) { omit(record, 'needs testdriver'); continue; }
        if (H2_NAME.test(path.basename(file)) && !http2) { omit(record, NEEDS_H2); continue; }
        // For listings only metadata is needed. Before execution, verify each
        // selected source, including its original bytes and upstream variant.
        if (selected && !verified) {
          const bytes = fs.readFileSync(path.join(wpt, source));
          if (crypto.createHash('sha1').update('blob ' + bytes.length + '\0').update(bytes).digest('hex') !== blob) {
            throw new Error('testharness manifest does not match source: ' + source);
          }
          verified = true;
        }
        const long = meta.timeout === 'long' || source.startsWith('wasm/core/');
        tests.push({ ...record, long, viewport: WPTRUNNER_VIEWPORT });
      }
    }
  }
  visit(manifest.items.testharness, '');
  if (unmatched.size) throw new Error('selected URL outside the manifest or requested directories: ' + [...unmatched][0]);
  return { tests, excluded, info: { inventoryOnly: omissions } };
}
