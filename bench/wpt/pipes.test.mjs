// bun test bench/wpt/pipes.test.mjs
import { expect, test } from 'bun:test';
import http from 'node:http';
import zlib from 'node:zlib';
import { parsePipes, pipeline, send, trickleSteps } from './pipes.mjs';

const body = Buffer.from('hello world\n');
const piped = (query, r = {}) => pipeline(new URL(`http://x/a.txt${query}`), { status: 200, headers: { 'content-type': 'text/plain' }, body, ...r });

test('the pipe string parses as wptserve tokenizes it', () => {
  expect(parsePipes('trickle(d1)')).toEqual([['trickle', ['d1']]]);
  expect(parsePipes('gzip|trickle(45000:d1:r2)')).toEqual([['gzip', []], ['trickle', ['45000:d1:r2']]]);
  // Empty functions between bars are skipped; escapes; a trailing bar.
  expect(parsePipes('header(a,b\\,c)||header(d,\\n\\))|')).toEqual([['header', ['a', 'b,c']], ['header', ['d', '\n)']]]);
  // An argument runs to the end of the string when nothing closes it.
  expect(parsePipes('header(d,e\\)|')).toEqual([['header', ['d', 'e)|']]]);
  expect(parsePipes('slice(1,null)status(404)')).toEqual([['slice', ['1', 'null']], ['status', ['404']]]);
});

test('only the last pipe parameter counts, and sub is not applied', () => {
  const r = piped('?pipe=status(404)&x=1&pipe=sub|status(410)');
  expect(r.status).toBe(410);
  expect(r.body).toBe(body);
  expect(piped('?pipe=')).toEqual(piped(''));
  expect(piped('?pipe=status(404)&pipe=').status).toBe(404);
});

test('trickle steps follow add_content', () => {
  const steps = (spec, size = body.length) => [...trickleSteps(piped(`?pipe=trickle(${spec})`).trickle, size)];
  expect(steps('d1')).toEqual([['delay', 1], ['bytes', 0, 12]]);
  // Adjacent steps of one kind add up.
  expect(steps('d0.5:d1:3:4')).toEqual([['delay', 1.5], ['bytes', 0, 7], ['bytes', 7, 12]]);
  // A last rN repeats the N steps before it until the body is sent; nothing is sent after the repeats.
  expect(steps('5:d0.5:r2')).toEqual([['bytes', 0, 5], ['delay', 0.5], ['bytes', 5, 10], ['delay', 0.5], ['bytes', 10, 15], ['delay', 0.5]]);
  // An rN that is not last is ignored.
  expect(steps('r2:5')).toEqual([['bytes', 0, 5], ['bytes', 5, 12]]);
  // Repeated steps that send nothing: the rest follows (wptserve would loop forever).
  expect(steps('d1:r2')).toEqual([['delay', 1], ['bytes', 0, 12]]);
  expect(() => piped('?pipe=trickle(d1:r3)')).toThrow();
  expect(() => piped('?pipe=trickle(dx)')).toThrow();
});

test('trickle marks the response uncacheable unless it says otherwise', () => {
  expect(piped('?pipe=trickle(d1)').headers).toMatchObject({ 'cache-control': 'no-cache, no-store, must-revalidate', pragma: 'no-cache', expires: '0' });
  const kept = piped('?pipe=trickle(d1)', { headers: { pragma: 'x' } }).headers;
  expect(kept).toEqual({ pragma: 'x' });
});

test('status, header, slice and gzip', () => {
  const r = piped('?pipe=header(X-A,1)|header(x-a,2,true)|header(Content-Type,text/html)|slice(-6,null)|status(201)');
  expect(r.status).toBe(201);
  expect(r.headers).toEqual({ 'content-type': 'text/html', 'x-a': ['1', '2'], 'content-length': '6' });
  expect(r.body.toString()).toBe('world\n');
  const g = piped('?pipe=gzip');
  expect(zlib.gunzipSync(g.body).toString()).toBe(body.toString());
  expect(g.headers).toMatchObject({ 'content-encoding': 'gzip', 'content-length': String(g.body.length) });
});

test('a trickled response sends its headers at once and its body after the delay', async () => {
  const server = http.createServer((req, res) => send(res, piped('?pipe=trickle(3:d0.3)')));
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    const t0 = performance.now();
    const res = await fetch(`http://127.0.0.1:${server.address().port}/`);
    const headersAt = performance.now() - t0;
    expect(res.headers.get('content-length')).toBeNull();
    const reader = res.body.getReader();
    const first = await reader.read();
    expect(Buffer.from(first.value).toString()).toBe('hel');
    let rest = '';
    for (let c = await reader.read(); !c.done; c = await reader.read()) rest += Buffer.from(c.value).toString();
    const bodyAt = performance.now() - t0;
    expect(rest).toBe('lo world\n');
    expect(headersAt).toBeLessThan(200);
    expect(bodyAt).toBeGreaterThanOrEqual(290);
  } finally {
    server.close();
  }
});
