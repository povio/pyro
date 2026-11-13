// Playwright's CDPSession: raw CDP on a page's session of pyro.
import { EventEmitter } from 'node:events';
import type { Cdp } from '../cdp.js';

export class CDPSession {
  private _events = new EventEmitter();
  private _off = new Map<string, () => void>();
  private _detached = false;
  constructor(
    private _cdp: Cdp,
    private _sessionId: string,
  ) {}
  async send<T = any>(method: string, params: object = {}): Promise<T> {
    if (this._detached) throw new Error('cdpSession.send: Session closed');
    return this._cdp.send<T>(method, params, this._sessionId);
  }
  on(event: string, listener: (params: any) => void): this {
    if (!this._off.has(event)) {
      this._off.set(event, this._cdp.on(event, (p, sid) => sid === this._sessionId && this._events.emit(event, p)));
    }
    this._events.on(event, listener);
    return this;
  }
  addListener(event: string, listener: (params: any) => void): this {
    return this.on(event, listener);
  }
  once(event: string, listener: (params: any) => void): this {
    const once = (p: any) => {
      this.off(event, once);
      listener(p);
    };
    return this.on(event, once);
  }
  off(event: string, listener: (params: any) => void): this {
    this._events.off(event, listener);
    return this;
  }
  removeListener(event: string, listener: (params: any) => void): this {
    return this.off(event, listener);
  }
  async detach(): Promise<void> {
    this._detached = true;
    for (const off of this._off.values()) off();
    this._off.clear();
  }
}
