// Baselines on disk for `matchScreenshot` steps. Before a run: read each
// baseline (`<dir>/<name>.png` and its `<name>.json` with the display-list
// hash) and attach it, unless this connection already has those bytes. After
// it: write recorded images (missing baseline, update mode) and, on a
// mismatch, `<name>.actual.png` and `<name>.diff.png`.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { ScreenshotStep } from './steps.js';

/** Playwright's `updateSnapshots` modes, for `toHaveScreenshot`. */
export type UpdateMode = 'all' | 'changed' | 'missing' | 'none';

/**
 * Write new baselines instead of comparing (`LP_UPDATE_SNAPSHOTS=1`, or
 * `bun test --update-snapshots` / `-u`). The steps API's switch; the
 * Playwright layer passes its own `UpdateMode` to `prepare`/`settle`.
 */
export const updateMode = (): boolean =>
  ['1', 'true', 'all', 'changed'].includes(process.env.LP_UPDATE_SNAPSHOTS ?? '') ||
  process.argv.includes('--update-snapshots') ||
  process.argv.includes('-u');

export type ScreenshotResult = {
  name: string;
  pass: boolean;
  /** Passed on the display-list hash: no raster, no pixel compare. */
  hashHit: boolean;
  /** No baseline was compared: the render was written as the baseline. */
  recorded: boolean;
  diffPixels: number;
  sceneHash: string;
  width: number;
  height: number;
  /** Browser-side timings, ms. */
  ms: Record<string, number>;
  /** The written baseline (recorded) or the images written on a mismatch. */
  files?: { baseline?: string; actual?: string; diff?: string };
};

export class ScreenshotMismatchError extends Error {
  constructor(
    message: string,
    readonly result: ScreenshotResult,
  ) {
    super(message);
  }
}

/** A baseline as this connection last received it, by wire name. */
export type SentBaselines = Map<string, string>;

const paths = (step: ScreenshotStep) => {
  const base = join(step.opts.dir ?? join(step.testDir, '__screenshots__'), step.name);
  if (step.outputStem) {
    const out = step.outputStem;
    return { png: `${base}.png`, meta: `${base}.json`, actual: `${out}-actual.png`, diff: `${out}-diff.png`, expected: `${out}-expected.png` };
  }
  return { png: `${base}.png`, meta: `${base}.json`, actual: `${base}.actual.png`, diff: `${base}.diff.png`, expected: undefined as string | undefined };
};

/**
 * Fills `step.wire` from disk. With a `mode` (the Playwright layer), only
 * "all" or a missing baseline records; "changed" compares, and the caller
 * rewrites on a mismatch (`rewrite`).
 */
export function prepare(step: ScreenshotStep, sent: SentBaselines, mode?: UpdateMode): void {
  const p = paths(step);
  step.wire = {};
  if ((mode ? mode === 'all' : updateMode()) || !existsSync(p.png)) {
    step.wire = { update: true };
    return;
  }
  const png = readFileSync(p.png);
  const digest = createHash('sha1').update(png).digest('hex');
  const key = `${p.png}`;
  // The wire name is the baseline's path, so two tests' `home` never share a cache entry.
  step.wire.name = key;
  if (sent.get(key) !== digest) {
    step.wire.baseline = png.toString('base64');
    try {
      const meta = JSON.parse(readFileSync(p.meta, 'utf8')) as { sceneHash?: string };
      if (meta.sceneHash) step.wire.sceneHash = meta.sceneHash;
    } catch {}
    sent.set(key, digest);
  }
}

const toResult = (step: ScreenshotStep, r: any): ScreenshotResult => ({
  name: step.name,
  pass: r.pass,
  hashHit: r.hashHit,
  recorded: r.recorded,
  diffPixels: r.diffPixels,
  sceneHash: r.sceneHash,
  width: r.width,
  height: r.height,
  ms: r.ms,
});

/**
 * A step's result from a successful run: writes recorded baselines. Without
 * a `mode` it throws on a recorded baseline outside update mode; with one,
 * the caller reports it (`result.recorded`).
 */
export function settle(step: ScreenshotStep, r: any, mode?: UpdateMode): ScreenshotResult {
  const p = paths(step);
  const result = toResult(step, r);
  rmSync(p.actual, { force: true });
  rmSync(p.diff, { force: true });
  if (r.recorded) {
    const existed = existsSync(p.png);
    mkdirSync(dirname(p.png), { recursive: true });
    writeFileSync(p.png, Buffer.from(r.actual, 'base64'));
    writeFileSync(p.meta, JSON.stringify({ sceneHash: r.sceneHash, width: r.width, height: r.height }, null, 2) + '\n');
    result.files = { baseline: p.png };
    if (!mode && !updateMode()) {
      const what = existed ? 'was replaced' : 'did not exist';
      throw new ScreenshotMismatchError(`screenshot '${step.name}': the baseline ${what}; wrote ${p.png}`, result);
    }
  }
  return result;
}

/** On a mismatch in "changed" / "all" mode: the actual image becomes the baseline. */
export function rewrite(step: ScreenshotStep, data: any): ScreenshotResult {
  const p = paths(step);
  const result = toResult(step, data);
  mkdirSync(dirname(p.png), { recursive: true });
  writeFileSync(p.png, Buffer.from(data.actual, 'base64'));
  writeFileSync(p.meta, JSON.stringify({ sceneHash: data.sceneHash, width: data.width, height: data.height }, null, 2) + '\n');
  rmSync(p.actual, { force: true });
  rmSync(p.diff, { force: true });
  result.files = { baseline: p.png };
  return result;
}

/** A failed run's `error.data` for this step: writes the actual image and the diff. */
export function mismatch(step: ScreenshotStep, data: any, message: string): ScreenshotMismatchError {
  const p = paths(step);
  const result = toResult(step, data);
  mkdirSync(dirname(p.actual), { recursive: true });
  writeFileSync(p.actual, Buffer.from(data.actual, 'base64'));
  writeFileSync(p.diff, Buffer.from(data.diff, 'base64'));
  if (p.expected && existsSync(p.png)) writeFileSync(p.expected, readFileSync(p.png));
  result.files = { baseline: p.png, actual: p.actual, diff: p.diff };
  return new ScreenshotMismatchError(`${message}\n  actual: ${p.actual}\n  diff:   ${p.diff}`, result);
}
