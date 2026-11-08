// Minimal CDP client over the runtime's WebSocket (Bun, Node >= 22): flat
// sessions, one promise per command, and event listeners by method.

export class CdpError extends Error {
  constructor(
    readonly method: string,
    readonly code: number,
    message: string,
    /** The error's `data` (a failed `matchScreenshot` carries its images). */
    readonly data?: unknown,
  ) {
    super(`${method}: ${message}`);
  }
}

type Pending = { method: string; resolve: (v: any) => void; reject: (e: Error) => void };
type Listener = (params: any, sessionId?: string) => void;

export class Cdp {
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private closed: Error | null = null;
  private listeners = new Map<string, Set<Listener>>();
  private closeListeners = new Set<(error: Error) => void>();

  private constructor(private ws: WebSocket) {
    ws.addEventListener('message', (ev) => this.onMessage(String(ev.data)));
    ws.addEventListener('close', () => this.fail(new Error('CDP connection closed')));
    ws.addEventListener('error', () => this.fail(new Error('CDP connection failed')));
  }

  static open(url: string): Promise<Cdp> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.addEventListener('open', () => resolve(new Cdp(ws)), { once: true });
      ws.addEventListener('error', () => reject(new Error(`cannot connect to ${url}`)), { once: true });
    });
  }

  send<T = any>(method: string, params: object = {}, sessionId?: string): Promise<T> {
    if (this.closed) return Promise.reject(this.closed);
    const id = this.nextId++;
    const msg: Record<string, unknown> = { id, method, params };
    if (sessionId) msg.sessionId = sessionId;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { method, resolve, reject });
      this.ws.send(JSON.stringify(msg));
    });
  }

  /** Sends without waiting; a failure is ignored (cleanup). */
  post(method: string, params: object = {}, sessionId?: string): void {
    this.send(method, params, sessionId).catch(() => {});
  }

  /** Calls `fn` for every `method` event (any session); returns the unsubscribe. */
  on(method: string, fn: Listener): () => void {
    let set = this.listeners.get(method);
    if (!set) this.listeners.set(method, (set = new Set()));
    set.add(fn);
    return () => set.delete(fn);
  }

  /** Calls `fn` once when the connection closes or fails. */
  onClose(fn: (error: Error) => void): void {
    if (this.closed) fn(this.closed);
    else this.closeListeners.add(fn);
  }

  get isClosed(): boolean {
    return this.closed !== null;
  }

  close(): void {
    this.ws.close();
  }

  private onMessage(text: string) {
    const msg = JSON.parse(text);
    if (typeof msg.id !== 'number') {
      const set = typeof msg.method === 'string' ? this.listeners.get(msg.method) : undefined;
      if (set) for (const fn of [...set]) fn(msg.params, msg.sessionId);
      return;
    }
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    if (msg.error) p.reject(new CdpError(p.method, msg.error.code, msg.error.message, msg.error.data));
    else p.resolve(msg.result);
  }

  private fail(error: Error) {
    if (this.closed) return;
    this.closed = error;
    for (const p of this.pending.values()) p.reject(error);
    this.pending.clear();
    for (const fn of this.closeListeners) fn(error);
    this.closeListeners.clear();
  }
}
