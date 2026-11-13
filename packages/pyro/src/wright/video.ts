// Playwright's video recording (`recordVideo`) on pyro. Each page of such a
// context is screencast (`Page.startScreencast`: pyro renders a JPEG frame
// when what the page shows changes, at most 25 a second) into a temporary
// directory. When the page closes, ffmpeg encodes the frames into
// `<dir>/<guid>.webm` as Playwright's recorder does (VP8 at a constant 25
// fps, each frame repeated until the next one's time, the last one held for
// at least a second, padded with gray to the video size) and the frames go.
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { accessSync, constants, mkdtempSync, readdirSync, rmSync, statSync } from 'node:fs';
import { copyFile, mkdir, readFile, rm, unlink, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { delimiter, dirname, join, resolve } from 'node:path';
import type { Cdp } from '../cdp.js';

type Size = { width: number; height: number };
/** `recordVideo` as `browser.newContext()` takes it. */
export type RecordVideoOptions = {
  /** Where the videos go (default: a temporary directory, removed when the process exits). */
  dir?: string;
  /** Default: the viewport scaled down to fit 800x800 (800x600 without one). */
  size?: Size;
  /** Accepted; no annotations are drawn. */
  showActions?: { duration?: number; position?: 'top-left' | 'top' | 'top-right' | 'bottom-left' | 'bottom' | 'bottom-right'; fontSize?: number };
};

const FPS = 25;

/**
 * The ffmpeg videos are encoded with: `$PYRO_FFMPEG`, else `ffmpeg` on PATH,
 * else Playwright's (`playwright install ffmpeg`, in its browsers directory).
 */
export function findFfmpeg(): string | undefined {
  const own = process.env.PYRO_FFMPEG;
  if (own) return executable(own) ? resolve(own) : undefined;
  const exe = process.platform === 'win32' ? 'ffmpeg.exe' : 'ffmpeg';
  for (const dir of (process.env.PATH ?? '').split(delimiter)) {
    if (dir && executable(join(dir, exe))) return join(dir, exe);
  }
  return playwrightFfmpeg();
}

/** Playwright's own ffmpeg, the newest one `playwright install ffmpeg` put in its browsers directory. */
export function playwrightFfmpeg(): string | undefined {
  const name = process.platform === 'darwin' ? 'ffmpeg-mac' : process.platform === 'win32' ? 'ffmpeg-win64.exe' : 'ffmpeg-linux';
  const root = playwrightBrowsers();
  let revisions: string[] = [];
  try {
    revisions = readdirSync(root).filter((d) => /^ffmpeg-\d+$/.test(d));
  } catch {}
  for (const d of revisions.sort((a, b) => Number(b.slice(7)) - Number(a.slice(7)))) {
    if (executable(join(root, d, name))) return join(root, d, name);
  }
  return undefined;
}

/** Playwright's browsers directory (its registry's rules). */
function playwrightBrowsers(): string {
  const env = process.env.PLAYWRIGHT_BROWSERS_PATH;
  if (env === '0') return join(process.cwd(), 'node_modules', 'playwright-core', '.local-browsers');
  if (env) return resolve(env);
  const home = homedir();
  if (process.platform === 'darwin') return join(home, 'Library', 'Caches', 'ms-playwright');
  if (process.platform === 'win32') return join(process.env.LOCALAPPDATA || join(home, 'AppData', 'Local'), 'ms-playwright');
  return join(process.env.XDG_CACHE_HOME || join(home, '.cache'), 'ms-playwright');
}

function executable(file: string): boolean {
  try {
    accessSync(file, constants.X_OK);
    return statSync(file).isFile();
  } catch {
    return false;
  }
}

/** Temporary directories this process made (frames, default video dir): removed when it exits. */
const temporary = new Set<string>();
let cleanupOnExit = false;
function tempDir(prefix: string): string {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  temporary.add(dir);
  if (!cleanupOnExit) {
    cleanupOnExit = true;
    // Synchronous: `exit` runs no promise callbacks.
    process.on('exit', () => {
      for (const d of temporary) rmSync(d, { recursive: true, force: true });
    });
  }
  return dir;
}
let defaultDir: string | undefined;

/** The videos of one context's pages. */
export class VideoRecorder {
  readonly videos: Video[] = [];
  private constructor(
    readonly ffmpeg: string,
    readonly dir: string,
    /** The video's. */
    readonly size: Size,
    /** What frames are scaled down to fit (`maxWidth`, `maxHeight`). */
    readonly frames: Size,
  ) {}

  /** As Playwright sizes videos; throws when there is no ffmpeg to encode them with. */
  static create(o: RecordVideoOptions, viewport: Size | null): VideoRecorder {
    const ffmpeg = findFfmpeg();
    if (!ffmpeg) {
      const why = process.env.PYRO_FFMPEG ? `PYRO_FFMPEG=${process.env.PYRO_FFMPEG} is not an executable` : `no ffmpeg on PATH or in ${playwrightBrowsers()}`;
      throw new Error(
        `browser.newContext: recordVideo encodes videos with ffmpeg, and there is none (${why}). ` +
          `Install one (macOS: brew install ffmpeg; Debian or Ubuntu: apt-get install ffmpeg; anywhere: bunx playwright install ffmpeg) or set PYRO_FFMPEG to one.`,
      );
    }
    const dir = o.dir ? resolve(o.dir) : (defaultDir ??= tempDir('pyro-videos-'));
    const v = viewport ?? { width: 800, height: 600 };
    const scale = Math.min(1, 800 / Math.max(v.width, v.height));
    const wanted = o.size ?? { width: Math.floor(v.width * scale), height: Math.floor(v.height * scale) };
    // As Playwright 1.59: frames fit the size with even sides; the video is
    // that, or the size as given (padded with gray).
    const frames = { width: wanted.width & ~1, height: wanted.height & ~1 };
    return new VideoRecorder(ffmpeg, dir, o.size ? { width: o.size.width, height: o.size.height } : frames, frames);
  }

  /** The video of a new page of the context. */
  add(): Video {
    const video = new Video(this);
    this.videos.push(video);
    return video;
  }

  /** Every page's video written (their pages closed). */
  async finish(): Promise<void> {
    await Promise.all(this.videos.map((v) => v._stop()));
  }
}

/**
 * Playwright's Video: a page's recording. `path()` is known from the start;
 * the file is there once the page closed (`page.close()` and
 * `context.close()` return after it is written).
 */
export class Video {
  private _path: string;
  private _frames: { file: string; time: number }[] = [];
  private _framesDir?: string;
  private _writes: Promise<unknown> = Promise.resolve();
  /** When the last frame came (`performance.now()`). */
  private _lastFrameAt = 0;
  private _off?: () => void;
  private _stopping?: Promise<void>;
  private _written!: () => void;
  private _finished = new Promise<void>((resolve) => (this._written = resolve));
  private _failure?: Error;
  private _deleted = false;
  /** `path()` or `saveAs()` was called: the file is wanted whatever the test runner's video mode. */
  private _wanted = false;
  private _discarded = false;

  constructor(private _recorder: VideoRecorder) {
    this._path = join(_recorder.dir, `${randomBytes(16).toString('hex')}.webm`);
  }

  /** Where the video is written once its page closes. */
  async path(): Promise<string> {
    this._wanted = true;
    return this._path;
  }

  /** Copies the video to `path` once it is written: waits for the page to close. */
  async saveAs(path: string): Promise<void> {
    this._wanted = true;
    await this._finished;
    if (this._deleted) throw new Error('File already deleted. Save before deleting.');
    if (this._failure) throw this._failure;
    await mkdir(dirname(resolve(path)), { recursive: true });
    await copyFile(this._path, path);
  }

  /** Deletes the video file, once it is written. */
  async delete(): Promise<void> {
    await this._finished;
    if (this._deleted) return;
    this._deleted = true;
    await unlink(this._path).catch(() => {});
  }

  /** @internal Starts the page's screencast on its new session. */
  _start(cdp: Cdp, sessionId: string): Promise<unknown> {
    if (this._stopping) return Promise.resolve();
    this._off?.();
    this._off = cdp.on('Page.screencastFrame', (p, sid) => {
      if (sid !== sessionId) return;
      cdp.post('Page.screencastFrameAck', { sessionId: p.sessionId }, sessionId);
      this._frame(Buffer.from(p.data, 'base64'), typeof p.metadata?.timestamp === 'number' ? p.metadata.timestamp : Date.now() / 1000, p.sessionId);
    });
    this._screencast = () => {
      const { width, height } = this._recorder.frames;
      return cdp.send('Page.startScreencast', { format: 'jpeg', quality: 90, maxWidth: width, maxHeight: height }, sessionId);
    };
    return this._screencast();
  }
  /** Starts (again) the page's screencast, once it has a session. */
  private _screencast?: () => Promise<unknown>;
  /** The screencast session of the last frame, and who waits for a frame of another. */
  private _castId?: number;
  private _waiting?: () => void;

  private _frame(jpeg: Buffer, time: number, castId: number): void {
    if (this._stopping) return;
    this._framesDir ??= tempDir('pyro-video-');
    const file = join(this._framesDir, `${String(this._frames.length).padStart(6, '0')}.jpg`);
    this._frames.push({ file, time });
    this._lastFrameAt = performance.now();
    this._writes = this._writes.then(() => writeFile(file, jpeg)).catch((e) => (this._failure ??= e));
    if (castId !== this._castId) this._waiting?.();
    this._castId = castId;
  }

  /**
   * @internal Before its page closes: a frame of what it shows now, which
   * the screencast may not have sent yet (the result of the test's last
   * action). A screencast started again sends one at once.
   */
  async _lastFrame(): Promise<void> {
    if (!this._screencast || this._stopping || this._discarded) return;
    this._castId ??= -1;
    await new Promise<void>((resolve) => {
      const timer = setTimeout(resolve, 1000);
      this._waiting = () => {
        clearTimeout(timer);
        resolve();
      };
      this._screencast!().catch(() => this._waiting?.());
    });
    this._waiting = undefined;
  }

  /** @internal The page closed: the video is written (once); never rejects. */
  _stop(): Promise<void> {
    return (this._stopping ??= this._encode());
  }

  /** @internal The test runner keeps no video of this page: nothing is encoded, unless the test asked for the file. */
  _discard(): void {
    if (!this._wanted && !this._stopping) this._discarded = true;
  }

  private async _encode(): Promise<void> {
    this._off?.();
    const end = performance.now();
    await this._writes;
    try {
      if (this._discarded) {
        this._deleted = true;
        return;
      }
      if (this._failure) return;
      await mkdir(dirname(this._path), { recursive: true });
      const { ffmpeg, size } = this._recorder;
      if (!this._frames.length) {
        // Nothing was shown (the page never got a target): white, as Playwright's recorder.
        await encode(ffmpeg, size, this._path, [{ data: async () => WHITE, count: FPS }], true);
        return;
      }
      // Playwright's frame numbers: a frame shows until the next one's slot;
      // the last one for the time since it came, at least a second.
      const t0 = this._frames[0].time;
      const slot = (t: number) => Math.floor((t - t0) * FPS);
      const last = this._frames[this._frames.length - 1];
      const until = [...this._frames.slice(1).map((f) => f.time), last.time + Math.max((end - this._lastFrameAt) / 1000, 1)];
      const frames = this._frames.map((f, i) => ({ data: () => readFile(f.file), count: Math.max(0, slot(until[i]) - slot(f.time)) }));
      await encode(ffmpeg, size, this._path, frames, false);
    } catch (e) {
      this._failure = e instanceof Error ? e : new Error(String(e));
    } finally {
      if (this._framesDir) {
        temporary.delete(this._framesDir);
        await rm(this._framesDir, { recursive: true, force: true }).catch(() => {});
      }
      this._written();
    }
  }
}

/**
 * Playwright's ffmpeg arguments: JPEGs piped in at 25 fps, VP8 WebM out,
 * padded with gray and cropped to `size` (`scale`: stretched to it first).
 * Padding in 4:4:4 (pyro's JPEGs are) works for odd sizes too.
 */
async function encode(ffmpeg: string, size: Size, out: string, frames: { data: () => Promise<Buffer>; count: number }[], scale: boolean): Promise<void> {
  const { width: w, height: h } = size;
  const vf = `${scale ? `scale=${w}:${h},` : ''}format=yuvj444p,pad=${w}:${h}:0:0:gray,crop=${w}:${h}:0:0`;
  const args = [
    ...'-loglevel error -f image2pipe -avioflags direct -fpsprobesize 0 -probesize 32 -analyzeduration 0 -c:v mjpeg -i pipe:0 -y -an'.split(' '),
    ...`-r ${FPS} -c:v vp8 -qmin 0 -qmax 50 -crf 8 -deadline realtime -speed 8 -b:v 1M -threads 1`.split(' '),
    '-vf',
    vf,
    out,
  ];
  const child = spawn(ffmpeg, args, { stdio: ['pipe', 'ignore', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (d) => (stderr += d));
  let exited = false;
  const done = new Promise<number>((resolve, reject) => {
    child.on('error', (e) => {
      exited = true;
      reject(e);
    });
    child.on('close', (code) => {
      exited = true;
      resolve(code ?? 1);
    });
  });
  done.catch(() => {}); // awaited below
  // An ffmpeg that stops reading reports why through its exit code.
  child.stdin.on('error', () => {});
  const write = (data: Buffer) =>
    child.stdin.write(data) || exited
      ? Promise.resolve()
      : new Promise<void>((resolve) => {
          child.stdin.once('drain', resolve);
          done.then(() => resolve(), () => resolve());
        });
  for (const f of frames) {
    if (!f.count || exited) continue;
    const data = await f.data();
    for (let i = 0; i < f.count && !exited; i++) await write(data);
  }
  child.stdin.end();
  const code = await done;
  if (code !== 0) throw new Error(`ffmpeg (${ffmpeg}) could not write ${out}: exit code ${code}${stderr.trim() ? `: ${stderr.trim()}` : ''}`);
}

/** A 16x16 white JPEG (scaled to the video's size). */
const WHITE = Buffer.from(
  '/9j/4AAQSkZJRgABAgAAAQABAAD//gAPTGF2YzYzLjEuMTAyAP/bAEMACAQEBAQEBQUFBQUFBgYGBgYGBgYGBgYGBgcHBwgICAcHBwYGBwcICAgICQkJCAgICAkJCgoKDAwLCw4ODhERFP/EAEsAAQEAAAAAAAAAAAAAAAAAAAAHAQEAAAAAAAAAAAAAAAAAAAAAEAEAAAAAAAAAAAAAAAAAAAAAEQEAAAAAAAAAAAAAAAAAAAAA/8AAEQgAEAAQAwEiAAIRAAMRAP/aAAwDAQACEQMRAD8Av4AP/9k=',
  'base64',
);
