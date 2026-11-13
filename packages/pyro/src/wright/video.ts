// Playwright's video recording (`recordVideo`) on pyro. Each page of such a
// context is screencast (`Page.startScreencast`: pyro renders a frame when
// what the page shows changes, at most 25 a second) and ffmpeg encodes the
// frames into `<dir>/<guid>.webm` as Playwright's recorder does (VP8 at a
// constant 25 fps, each frame repeated until the next one's time, the last
// one held for at least a second, padded with gray to the video size).
//
// With an ffmpeg that reads raw video (Homebrew's, Linux distributions'),
// pyro writes the frames raw (`yuv420p`) to a Unix socket here and ffmpeg
// encodes them as they come, so nothing is JPEG-encoded or decoded and the
// file is ready soon after the page closes. With Playwright's own ffmpeg
// (MJPEG input only), or when the video may be thrown away (the runner's
// `retain-on-failure`), pyro sends JPEG frames, which wait in a temporary
// directory and are encoded when the page closes (not at all when thrown
// away).
import { spawn, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { accessSync, constants, mkdtempSync, readdirSync, rmSync, statSync } from 'node:fs';
import { copyFile, mkdir, readFile, rm, unlink, writeFile } from 'node:fs/promises';
import { createServer, type Server, type Socket } from 'node:net';
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
 * VP8's bitrate cap (kbit/s): Playwright's 1M (for its ~800x450 default)
 * scaled with the frame's pixels, at most 32M. ffmpeg's `-crf` makes it a cap,
 * not a target: a 3840x2160 page with a moving box at 1M came out at 35 dB
 * PSNR (blurred text), at 16M at 51.5 dB in fewer bytes than at 8M.
 */
export function bitrate(size: Size): number {
  return Math.min(32_000, Math.max(1000, Math.round((size.width * size.height) / 360)));
}

/** Whether `ffmpeg` reads raw video (`-f rawvideo`): full builds do; Playwright's reads MJPEG only. */
const rawVideo = new Map<string, Promise<boolean>>();
export function readsRawVideo(ffmpeg: string): Promise<boolean> {
  let reads = rawVideo.get(ffmpeg);
  if (!reads) {
    const lists = (what: string, line: RegExp) =>
      new Promise<boolean>((resolve) => {
        const child = spawn(ffmpeg, ['-hide_banner', what], { stdio: ['ignore', 'pipe', 'ignore'] });
        let out = '';
        child.stdout.on('data', (d) => (out += d));
        child.on('error', () => resolve(false));
        child.on('close', (code) => resolve(code === 0 && line.test(out)));
      });
    reads = Promise.all([lists('-demuxers', /^ *D\S* +rawvideo /m), lists('-decoders', /^ *V\S* +rawvideo /m)]).then(([a, b]) => a && b);
    rawVideo.set(ffmpeg, reads);
  }
  return reads;
}

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
  /**
   * Raw frames encoded as they come, when ffmpeg reads raw video (see the
   * top of this file); off: JPEG frames encoded when the page closes. The
   * runner turns it off for videos it may throw away, which then cost no
   * encoding.
   */
  live = true;
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
  /** JPEG frames waiting in `_framesDir` (not live). */
  private _frames: { file: string; time: number }[] = [];
  private _framesDir?: string;
  /** Live: ffmpeg encoding the raw frames pyro writes to its socket. */
  private _live?: Live;
  private _liveStarting?: Promise<Live | undefined>;
  /** Live: the last frame handed to ffmpeg, and the first one's time. */
  private _shown?: { data: Buffer; time: number };
  private _t0 = 0;
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
    this._screencast = async () => {
      const { width, height } = this._recorder.frames;
      const live = await (this._liveStarting ??= this._startLive());
      if (live && live === this._live) {
        try {
          return await cdp.send('Page.startScreencast', { format: 'yuv420p', socket: live.frames.path, maxWidth: width, maxHeight: height }, sessionId);
        } catch (e) {
          if (this._shown) throw e;
          // A pyro without raw frames: JPEG frames, for the context's other videos too.
          this._recorder.live = false;
          this._endLive();
        }
      }
      return cdp.send('Page.startScreencast', { format: 'jpeg', quality: 90, maxWidth: width, maxHeight: height }, sessionId);
    };
    return this._screencast();
  }
  /** Starts (again) the page's screencast, once it has a session. */
  private _screencast?: () => Promise<unknown>;
  /** The screencast of the last frame (its session id, or its socket connection), and who waits for a frame of another. */
  private _castId?: number;
  private _waiting?: () => void;

  private _frame(jpeg: Buffer, time: number, castId: number): void {
    if (this._stopping) return;
    this._framesDir ??= tempDir('pyro-video-');
    const file = join(this._framesDir, `${String(this._frames.length).padStart(6, '0')}.jpg`);
    this._frames.push({ file, time });
    this._lastFrameAt = performance.now();
    this._writes = this._writes.then(() => writeFile(file, jpeg)).catch((e) => (this._failure ??= e));
    this._came(castId);
  }

  private _came(castId: number): void {
    if (castId !== this._castId) this._waiting?.();
    this._castId = castId;
  }

  /** Live, when the recorder and ffmpeg can: a socket for pyro's raw frames and ffmpeg encoding them. */
  private async _startLive(): Promise<Live | undefined> {
    const { ffmpeg, frames, size } = this._recorder;
    if (!this._recorder.live || !(await readsRawVideo(ffmpeg)) || this._stopping) return undefined;
    const dir = tempDir('pyro-video-');
    // A Unix socket path has room for 103 bytes on macOS.
    const path = join(dir, 's');
    const socket = Buffer.byteLength(path) <= 100 ? await FrameSocket.listen(path, (data, time, conn) => this._liveFrame(data, time, conn)).catch(() => undefined) : undefined;
    if (!socket || this._stopping) {
      socket?.close();
      temporary.delete(dir);
      rmSync(dir, { recursive: true, force: true });
      return undefined;
    }
    await mkdir(dirname(this._path), { recursive: true });
    // Distinct frames only, each at its frame slot: ffmpeg repeats them to 25 fps (`fps`).
    const pad = frames.width === size.width && frames.height === size.height ? '' : `,format=yuv444p,pad=${size.width}:${size.height}:0:0:gray,crop=${size.width}:${size.height}:0:0`;
    const args = [
      ...'-loglevel error -f matroska -probesize 32 -analyzeduration 0 -i pipe:0 -y -an'.split(' '),
      ...vp8(size),
      ...['-vf', `fps=${FPS}${pad}`, '-fps_mode', 'passthrough', this._path],
    ];
    const encoder = new Ffmpeg(ffmpeg, args);
    encoder.write(mkvHeader(frames));
    this._live = { frames: socket, encoder, dir, writing: Promise.resolve() };
    return this._live;
  }

  /** Live: the encoder and socket gone, the partial file too. */
  private _endLive(): void {
    const live = this._live;
    if (!live) return;
    this._live = undefined;
    live.frames.close();
    live.encoder.kill();
    temporary.delete(live.dir);
    rmSync(live.dir, { recursive: true, force: true });
    void unlink(this._path).catch(() => {});
  }

  /**
   * Live: a raw frame from pyro. It goes to ffmpeg once the last one did (so
   * at most two wait here); resolved, pyro gets its ack and renders the next.
   */
  private _liveFrame(data: Buffer, time: number, conn: number): Promise<void> {
    if (this._stopping) return Promise.resolve();
    this._lastFrameAt = performance.now();
    this._came(conn);
    const queued = this._writes.then(async () => {
      const live = this._live;
      // A frame of an earlier screencast, after a restart.
      if (!live || (this._shown && time <= this._shown.time)) return;
      if (!this._shown) this._t0 = time;
      this._shown = { data, time };
      await live.writing;
      live.writing = writeMkvFrame(live.encoder, this._slot(time), data);
    });
    this._writes = queued.catch((e) => (this._failure ??= e));
    return queued.catch(() => {});
  }

  /** A time's frame slot at 25 fps (Playwright's frame numbers). */
  private _slot(time: number): number {
    return Math.max(0, Math.floor((time - this._t0) * FPS));
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
    await this._liveStarting;
    await this._writes;
    const live = this._live;
    try {
      if (this._discarded) {
        this._deleted = true;
        this._endLive();
        return;
      }
      if (this._failure) return this._endLive();
      await mkdir(dirname(this._path), { recursive: true });
      const { ffmpeg, size } = this._recorder;
      if (live) {
        live.frames.close();
        await live.writing;
        const last = this._shown;
        if (last) {
          // The last frame shows for the time since it came, at least a second: its last slot gets a copy.
          const slot = this._slot(last.time + Math.max((end - this._lastFrameAt) / 1000, 1)) - 1;
          if (slot > this._slot(last.time)) await writeMkvFrame(live.encoder, slot, last.data);
        } else {
          // Nothing was shown: a second of white, as Playwright's recorder.
          const white = whiteFrame(this._recorder.frames);
          await writeMkvFrame(live.encoder, 0, white);
          await writeMkvFrame(live.encoder, FPS - 1, white);
        }
        await live.encoder.finish(this._path);
        return;
      }
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
      if (live) {
        live.encoder.kill();
        temporary.delete(live.dir);
        await rm(live.dir, { recursive: true, force: true }).catch(() => {});
      }
      if (this._framesDir) {
        temporary.delete(this._framesDir);
        await rm(this._framesDir, { recursive: true, force: true }).catch(() => {});
      }
      this._written();
    }
  }
}

type Live = {
  frames: FrameSocket;
  encoder: Ffmpeg;
  /** The socket's temporary directory. */
  dir: string;
  /** The last frame handed to ffmpeg: resolved once it took it. */
  writing: Promise<void>;
};

/** Playwright's VP8 settings, with the bitrate cap scaled to the size. */
function vp8(size: Size): string[] {
  return `-c:v vp8 -qmin 0 -qmax 50 -crf 8 -deadline realtime -speed 8 -b:v ${bitrate(size)}k -threads 1`.split(' ');
}

/** An ffmpeg fed on stdin. */
class Ffmpeg {
  private child: ChildProcess;
  private stderr = '';
  private exited = false;
  private done: Promise<number>;

  constructor(
    private readonly bin: string,
    args: string[],
  ) {
    this.child = spawn(bin, args, { stdio: ['pipe', 'ignore', 'pipe'] });
    this.child.stderr!.on('data', (d) => (this.stderr += d));
    this.done = new Promise<number>((resolve, reject) => {
      this.child.on('error', (e) => {
        this.exited = true;
        reject(e);
      });
      this.child.on('close', (code) => {
        this.exited = true;
        resolve(code ?? 1);
      });
    });
    this.done.catch(() => {}); // awaited in finish()
    // An ffmpeg that stops reading reports why through its exit code.
    this.child.stdin!.on('error', () => {});
  }

  /** Writes `data`; resolves once ffmpeg took it (or exited). */
  write(data: Buffer): Promise<void> {
    if (this.exited || this.child.stdin!.write(data)) return Promise.resolve();
    return new Promise<void>((resolve) => {
      this.child.stdin!.once('drain', resolve);
      this.done.then(() => resolve(), () => resolve());
    });
  }

  /** Ends the input; resolves once the file is written, throws when ffmpeg failed. */
  async finish(out: string): Promise<void> {
    this.child.stdin!.end();
    const code = await this.done;
    if (code !== 0) throw new Error(`ffmpeg (${this.bin}) could not write ${out}: exit code ${code}${this.stderr.trim() ? `: ${this.stderr.trim()}` : ''}`);
  }

  get running(): boolean {
    return !this.exited;
  }

  kill(): void {
    if (!this.exited) this.child.kill('SIGKILL');
  }
}

/**
 * The Unix socket pyro writes raw frames to (`Page.startScreencast` format
 * `yuv420p`), one connection per screencast start: per frame a 20-byte
 * header (`PYF1`, width and height as u32, the timestamp as f64, all
 * little-endian) and the frame (I420), acked with one byte once `onFrame`
 * resolves. Under Bun its own sockets read far faster (12 MB frames).
 */
class FrameSocket {
  private constructor(
    readonly path: string,
    private readonly stop: () => void,
  ) {}

  static async listen(path: string, onFrame: (data: Buffer, time: number, conn: number) => Promise<void>): Promise<FrameSocket> {
    let conns = 0;
    const reader = () => {
      const conn = ++conns;
      return new FrameReader((data, time) => onFrame(data, time, conn));
    };
    const bun = (globalThis as { Bun?: { listen: (o: object) => { stop: (force?: boolean) => void } } }).Bun;
    if (bun) {
      const server = bun.listen({
        unix: path,
        socket: {
          open: (s: { data?: FrameReader }) => void (s.data = reader()),
          data: (s: { data: FrameReader; write: (b: Uint8Array) => number; end: () => void }, chunk: Uint8Array) =>
            s.data.feed(Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength), () => s.write(ACK), () => s.end()),
        },
      });
      return new FrameSocket(path, () => server.stop(true));
    }
    const sockets = new Set<Socket>();
    const server: Server = createServer((socket) => {
      sockets.add(socket);
      socket.on('close', () => sockets.delete(socket));
      socket.on('error', () => {});
      const r = reader();
      socket.on('data', (chunk: Buffer) => r.feed(chunk, () => socket.write(ACK), () => socket.destroy()));
    });
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject);
      server.listen(path, () => resolve());
    });
    return new FrameSocket(path, () => {
      server.close();
      for (const s of sockets) s.destroy();
    });
  }

  close(): void {
    this.stop();
  }
}

const ACK = new Uint8Array([1]);

/** One connection's frames, parsed from its chunks. */
class FrameReader {
  private head = Buffer.alloc(20);
  private headLength = 0;
  private frame?: Buffer;
  private filled = 0;
  private time = 0;

  constructor(private readonly onFrame: (data: Buffer, time: number) => Promise<void>) {}

  /** `ack` once a frame is taken; `fail` on a stream that is not frames. */
  feed(chunk: Buffer, ack: () => void, fail: () => void): void {
    let at = 0;
    while (at < chunk.length) {
      if (!this.frame) {
        const n = Math.min(20 - this.headLength, chunk.length - at);
        chunk.copy(this.head, this.headLength, at, at + n);
        this.headLength += n;
        at += n;
        if (this.headLength < 20) return;
        this.headLength = 0;
        if (this.head.toString('latin1', 0, 4) !== 'PYF1') return fail();
        const w = this.head.readUInt32LE(4);
        const h = this.head.readUInt32LE(8);
        this.time = this.head.readDoubleLE(12);
        this.frame = Buffer.allocUnsafe(w * h + 2 * Math.ceil(w / 2) * Math.ceil(h / 2));
        this.filled = 0;
      }
      const n = Math.min(this.frame.length - this.filled, chunk.length - at);
      chunk.copy(this.frame, this.filled, at, at + n);
      this.filled += n;
      at += n;
      if (this.filled === this.frame.length) {
        const frame = this.frame;
        this.frame = undefined;
        // pyro sends the next frame only after the ack.
        void this.onFrame(frame, this.time).then(ack, ack);
      }
    }
  }
}

/** A frame of `size` all white (BT.601 limited range). */
function whiteFrame(size: Size): Buffer {
  const luma = size.width * size.height;
  const frame = Buffer.alloc(luma + 2 * Math.ceil(size.width / 2) * Math.ceil(size.height / 2), 128);
  frame.fill(235, 0, luma);
  return frame;
}

// A Matroska stream of raw frames for ffmpeg: one video track, I420 at the
// frames' size, 25 fps; each frame a cluster at its slot's time (ms).
function ebmlId(id: number): Buffer {
  const bytes: number[] = [];
  for (let n = id; n; n = Math.floor(n / 256)) bytes.unshift(n & 255);
  return Buffer.from(bytes);
}
function ebmlSize(n: number): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64BE(BigInt(n) | (1n << 56n));
  return b;
}
function ebmlUint(n: number): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64BE(BigInt(n));
  let i = 0;
  while (i < 7 && b[i] === 0) i++;
  return b.subarray(i);
}
function ebml(id: number, ...body: Buffer[]): Buffer {
  const data = Buffer.concat(body);
  return Buffer.concat([ebmlId(id), ebmlSize(data.length), data]);
}
function mkvHeader(size: Size): Buffer {
  const text = (s: string) => Buffer.from(s, 'latin1');
  const header = ebml(0x1a45dfa3, ebml(0x4286, ebmlUint(1)), ebml(0x42f7, ebmlUint(1)), ebml(0x42f2, ebmlUint(4)), ebml(0x42f3, ebmlUint(8)), ebml(0x4282, text('matroska')), ebml(0x4287, ebmlUint(4)), ebml(0x4285, ebmlUint(2)));
  const info = ebml(0x1549a966, ebml(0x2ad7b1, ebmlUint(1_000_000)), ebml(0x4d80, text('pyro')), ebml(0x5741, text('pyro')));
  const video = ebml(0xe0, ebml(0xb0, ebmlUint(size.width)), ebml(0xba, ebmlUint(size.height)), ebml(0x2eb524, text('I420')));
  const track = ebml(0xae, ebml(0xd7, ebmlUint(1)), ebml(0x73c5, ebmlUint(1)), ebml(0x83, ebmlUint(1)), ebml(0x86, text('V_UNCOMPRESSED')), ebml(0x23e383, ebmlUint(1e9 / FPS)), video);
  // The segment's size is unknown: it is a stream.
  return Buffer.concat([header, ebmlId(0x18538067), Buffer.from([0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]), info, ebml(0x1654ae6b, track)]);
}
/** Writes a cluster holding `frame` at frame slot `slot` (the frame itself is not copied). */
function writeMkvFrame(encoder: Ffmpeg, slot: number, frame: Buffer): Promise<void> {
  const time = ebml(0xe7, ebmlUint(Math.round((slot * 1000) / FPS)));
  // SimpleBlock: track 1, no time offset in the cluster, a key frame.
  const block = Buffer.concat([ebmlId(0xa3), ebmlSize(4 + frame.length), Buffer.from([0x81, 0, 0, 0x80])]);
  void encoder.write(Buffer.concat([ebmlId(0x1f43b675), ebmlSize(time.length + block.length + frame.length), time, block]));
  return encoder.write(frame);
}

/**
 * Playwright's ffmpeg arguments: JPEGs piped in at 25 fps, VP8 WebM out,
 * padded with gray and cropped to `size` (`scale`: stretched to it first).
 * Padding in 4:4:4 works for odd sizes too.
 */
async function encode(ffmpeg: string, size: Size, out: string, frames: { data: () => Promise<Buffer>; count: number }[], scale: boolean): Promise<void> {
  const { width: w, height: h } = size;
  const vf = `${scale ? `scale=${w}:${h},` : ''}format=yuvj444p,pad=${w}:${h}:0:0:gray,crop=${w}:${h}:0:0`;
  const args = [
    ...'-loglevel error -f image2pipe -avioflags direct -fpsprobesize 0 -probesize 32 -analyzeduration 0 -c:v mjpeg -i pipe:0 -y -an'.split(' '),
    ...`-r ${FPS}`.split(' '),
    ...vp8(size),
    '-vf',
    vf,
    out,
  ];
  const encoder = new Ffmpeg(ffmpeg, args);
  for (const f of frames) {
    if (!f.count || !encoder.running) continue;
    const data = await f.data();
    for (let i = 0; i < f.count && encoder.running; i++) await encoder.write(data);
  }
  await encoder.finish(out);
}

/** A 16x16 white JPEG (scaled to the video's size). */
const WHITE = Buffer.from(
  '/9j/4AAQSkZJRgABAgAAAQABAAD//gAPTGF2YzYzLjEuMTAyAP/bAEMACAQEBAQEBQUFBQUFBgYGBgYGBgYGBgYGBgcHBwgICAcHBwYGBwcICAgICQkJCAgICAkJCgoKDAwLCw4ODhERFP/EAEsAAQEAAAAAAAAAAAAAAAAAAAAHAQEAAAAAAAAAAAAAAAAAAAAAEAEAAAAAAAAAAAAAAAAAAAAAEQEAAAAAAAAAAAAAAAAAAAAA/8AAEQgAEAAQAwEiAAIRAAMRAP/aAAwDAQACEQMRAD8Av4AP/9k=',
  'base64',
);
