// Minimal CDP client over the runtime's WebSocket (Bun, Node >= 22): flat
// sessions, one promise per command, no event subscriptions.

export class CdpError extends Error {
  constructor(
    readonly method: string,
    readonly code: number,
    message: string,
  ) {
    super(`${method}: ${message}`);
  }
}

type Pending = { method: string; resolve: (v: any) => void; reject: (e: Error) => void };

export class Cdp {
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private closed: Error | null = null;

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

  close(): void {
    this.ws.close();
  }

  private onMessage(text: string) {
    const msg = JSON.parse(text);
    if (typeof msg.id !== 'number') return; // events are not used
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    if (msg.error) p.reject(new CdpError(p.method, msg.error.code, msg.error.message));
    else p.resolve(msg.result);
  }

  private fail(error: Error) {
    this.closed ??= error;
    for (const p of this.pending.values()) p.reject(error);
    this.pending.clear();
  }
}
