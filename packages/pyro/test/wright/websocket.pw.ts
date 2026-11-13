// page.on('websocket') and Playwright's WebSocket: frames both ways, close,
// and a failed connection, against a small RFC 6455 echo server.
import { createHash } from 'node:crypto';
import { createServer, type Server } from 'node:http';
import type { AddressInfo, Socket } from 'node:net';
import { expect, test } from './fixtures';

/** Unmasked server frame. */
function frame(opcode: number, payload: Buffer): Buffer {
  const head = payload.length < 126 ? Buffer.from([0x80 | opcode, payload.length]) : Buffer.from([0x80 | opcode, 126, payload.length >> 8, payload.length & 0xff]);
  return Buffer.concat([head, payload]);
}

/** Echoes text and binary messages; "bye" makes it close with 4001. */
function echo(socket: Socket) {
  let buf = Buffer.alloc(0);
  socket.on('data', (chunk) => {
    buf = Buffer.concat([buf, chunk]);
    while (buf.length >= 2) {
      const opcode = buf[0] & 0x0f;
      let len = buf[1] & 0x7f;
      let at = 2;
      if (len === 126) {
        if (buf.length < 4) return;
        len = buf.readUInt16BE(2);
        at = 4;
      }
      if (buf.length < at + 4 + len) return;
      const mask = buf.subarray(at, at + 4);
      const payload = Buffer.from(buf.subarray(at + 4, at + 4 + len).map((b, i) => b ^ mask[i % 4]));
      buf = buf.subarray(at + 4 + len);
      if (opcode === 8) {
        socket.end(frame(8, payload.subarray(0, 2)));
      } else if (opcode === 1 && payload.toString() === 'bye') {
        socket.write(frame(8, Buffer.concat([Buffer.from([0x0f, 0xa1]), Buffer.from('server says bye')])));
      } else if (opcode === 1 || opcode === 2) {
        socket.write(frame(opcode, payload));
      }
    }
  });
  socket.on('error', () => {});
}

let server: Server;
let port: number;

test.beforeAll(async () => {
  server = createServer((_req, res) => res.writeHead(404).end());
  server.on('upgrade', (req, socket: Socket) => {
    const accept = createHash('sha1').update(`${req.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest('base64');
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    socket.write(frame(1, Buffer.from('welcome')));
    echo(socket);
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  port = (server.address() as AddressInfo).port;
});

test.afterAll(() => {
  server.closeAllConnections();
  server.close();
});

test('page.on("websocket") sees frames both ways and the close', async ({ page }) => {
  await page.goto('/simple');
  const url = `ws://127.0.0.1:${port}/chat`;
  // Listeners go on in the 'websocket' handler: frames can arrive right after it.
  const sent: (string | Buffer)[] = [];
  const received: (string | Buffer)[] = [];
  let echoed!: Promise<unknown>;
  const wsPromise = new Promise<any>((resolve) => page.once('websocket', (ws) => {
    ws.on('framesent', (f: { payload: string | Buffer }) => sent.push(f.payload));
    ws.on('framereceived', (f: { payload: string | Buffer }) => received.push(f.payload));
    echoed = ws.waitForEvent('framereceived', (f: { payload: string | Buffer }) => Buffer.isBuffer(f.payload));
    resolve(ws);
  }));
  await page.evaluate((url) => {
    const w = window as any;
    w.got = [];
    w.ws = new WebSocket(url);
    w.ws.binaryType = 'arraybuffer';
    w.ws.onmessage = (e: MessageEvent) => w.got.push(typeof e.data === 'string' ? e.data : `bytes:${new Uint8Array(e.data).join(',')}`);
    w.ws.onopen = () => {
      w.ws.send('hello');
      w.ws.send(new Uint8Array([1, 2, 3]));
    };
  }, url);
  const ws = await wsPromise;
  expect(ws.url()).toBe(url);
  expect(ws.isClosed()).toBe(false);

  await echoed;
  await expect.poll(() => page.evaluate(() => (window as any).got.join(' '))).toBe('welcome hello bytes:1,2,3');
  expect(sent).toEqual(['hello', Buffer.from([1, 2, 3])]);
  expect(received).toEqual(['welcome', 'hello', Buffer.from([1, 2, 3])]);

  const closed = ws.waitForEvent('close');
  const code = page.evaluate(() => new Promise((r) => {
    const w = window as any;
    w.ws.onclose = (e: CloseEvent) => r(`${e.code} ${e.reason} ${e.wasClean}`);
    w.ws.send('bye');
  }));
  expect(await closed).toBe(ws);
  expect(ws.isClosed()).toBe(true);
  expect(await code).toBe('4001 server says bye true');
});

test('an open socket does not hold up actions', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`<button onclick="window.ws = new WebSocket('ws://127.0.0.1:${port}/'); this.textContent = 'opened'">Open</button>`);
  // Listeners go on in the 'websocket' handler: frames can arrive right after it.
  let greeted!: Promise<unknown>;
  const wsPromise = new Promise<any>((resolve) => page.once('websocket', (ws) => {
    greeted = ws.waitForEvent('framereceived');
    resolve(ws);
  }));
  const started = Date.now();
  await page.getByRole('button', { name: 'Open' }).click();
  await expect(page.getByRole('button')).toHaveText('opened');
  const ws = await wsPromise;
  expect((await greeted as { payload: string }).payload).toBe('welcome');
  await page.locator('button').click();
  expect(Date.now() - started).toBeLessThan(5000);
  expect(ws.isClosed()).toBe(false);
});

test('a refused connection reports socketerror, then close', async ({ page }) => {
  await page.goto('/simple');
  // A port nothing listens on.
  const probe = createServer();
  await new Promise<void>((r) => probe.listen(0, '127.0.0.1', r));
  const dead = (probe.address() as AddressInfo).port;
  await new Promise((r) => probe.close(r));

  const seen: string[] = [];
  const closed = new Promise<void>((resolve) => page.once('websocket', (ws) => {
    ws.on('socketerror', (message: string) => seen.push(`socketerror ${message}`));
    ws.on('close', () => {
      seen.push('close');
      resolve();
    });
  }));
  const events = await page.evaluate((url) => new Promise((resolve) => {
    const log: string[] = [];
    const ws = new WebSocket(url);
    ws.onerror = () => log.push('error');
    ws.onclose = (e) => resolve([...log, `close ${e.code} ${e.wasClean}`]);
  }), `ws://127.0.0.1:${dead}/`);
  expect(events).toEqual(['error', 'close 1006 false']);
  await closed;
  expect(seen).toHaveLength(2);
  expect(seen[0]).toContain('ERR_CONNECTION_REFUSED');
  expect(seen[1]).toBe('close');
});
