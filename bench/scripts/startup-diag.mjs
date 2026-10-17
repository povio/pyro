// Startup diagnostic: spawn -> first successful TCP connect -> first HTTP 200 on /json/version.
// Usage: node startup-diag.mjs <lightpanda> [runs=5]
import { spawn } from 'node:child_process';
import net from 'node:net';
import http from 'node:http';
const [BIN, RUNS = '5'] = process.argv.slice(2);
const FLAGS = ['--log-level', 'error', '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'];
const freePort = () => new Promise((r) => { const s = net.createServer(); s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => r(p)); }); });
const tryConnect = (port) => new Promise((r) => { const s = net.connect(port, '127.0.0.1'); s.once('connect', () => { s.destroy(); r(true); }); s.once('error', () => r(false)); });
const get = (port) => new Promise((r) => { const t = performance.now(); const q = http.get({ host: '127.0.0.1', port, path: '/json/version', agent: false }, (res) => { res.resume(); res.on('end', () => r({ status: res.statusCode, ms: performance.now() - t })); }); q.on('error', () => r({ status: 0, ms: performance.now() - t })); });
for (let i = 0; i < Number(RUNS); i++) {
  const port = await freePort();
  const t0 = performance.now();
  const cp = spawn(BIN, ['serve', '--host', '127.0.0.1', '--port', String(port), ...FLAGS], { stdio: 'ignore', env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } });
  const spawned = await new Promise((r) => cp.once('spawn', () => r(performance.now() - t0)));
  let attempts = 0; while (!(await tryConnect(port))) { attempts++; await new Promise((r) => setTimeout(r, 1)); }
  const tcp = performance.now() - t0;
  const g = await get(port);
  const http1 = performance.now() - t0;
  const g2 = await get(port);
  console.log(JSON.stringify({ run: i + 1, spawnEventMs: +spawned.toFixed(1), firstTcpConnectMs: +tcp.toFixed(1), refusedAttempts: attempts, firstHttpStatus: g.status, firstHttpReqMs: +g.ms.toFixed(1), readyMs: +http1.toFixed(1), secondHttpReqMs: +g2.ms.toFixed(1) }));
  cp.kill('SIGTERM'); await new Promise((r) => cp.once('exit', r)); await new Promise((r) => setTimeout(r, Number(process.env.GAP ?? 200)));
}
