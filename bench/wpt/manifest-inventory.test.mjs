import { expect, test } from 'bun:test';
import { spawnSync } from "node:child_process";
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { manifestInventory } from './manifest-inventory.mjs';

test('retains exact variants from the manifest and validates only selected sources', () => {
  const wpt = fs.mkdtempSync(path.join(os.tmpdir(), 'manifest-inventory-'));
  try {
    fs.mkdirSync(path.join(wpt, 'demo'));
    const bytes = Buffer.from('<title>UTF-16</title>', 'utf16le');
    const blob = crypto.createHash('sha1').update('blob ' + bytes.length + '\0').update(bytes).digest('hex');
    const source = path.join(wpt, 'demo/variant.html');
    fs.writeFileSync(source, bytes);
    const url = "demo/variant.html?q=''";
    const manifest = {url_base: '/', items: {testharness: {demo: {
      'variant.html': [blob, [url, {timeout: 'long'}]],
      'shell.any.js': ['unused', ['demo/shell.any.js', {jsshell: true}]],
      'bidi.html': ['unused', [null, {testdriver: true, testdriver_features: ['bidi']}]],
      'pac.html': ['unused', [null, {pac: '/proxy.pac'}]]
    }}}};
    const options = {dirs: ['demo'], http2: true, testdriver: true};
    const run = manifestInventory(wpt, manifest, options);
    expect(run.tests.map(t => [t.url, t.long])).toEqual([[url, true]]);
    expect(run.info.inventoryOnly.map(t => t.url).sort()).toEqual(['demo/bidi.html', 'demo/pac.html', 'demo/shell.any.js']);
    expect(run.tests[0].viewport).toEqual({ width: 800, height: 600 });
    const shell = run.info.inventoryOnly.find(t => t.url === 'demo/shell.any.js');
    expect(shell.environment).toBe('jsshell');
    expect(shell.source).toBe('demo/shell.any.js');
    fs.writeFileSync(source, 'changed');
    expect(() => manifestInventory(wpt, manifest, {...options, selected: new Set([url])})).toThrow('does not match source');
  } finally { fs.rmSync(wpt, {recursive: true, force: true}); }
});

test('list output and startup checkpoints describe the non-CSS plan and every environment', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'manifest-output-'));
  try {
    fs.mkdirSync(path.join(root, 'demo'));
    const body = '<title>ordinary test</title>';
    const blob = crypto.createHash('sha1').update('blob ' + body.length + '\0').update(body).digest('hex');
    fs.writeFileSync(path.join(root, 'demo/test.html'), body);
    const manifest = path.join(root, 'manifest.json');
    fs.writeFileSync(manifest, JSON.stringify({ url_base:'/', items:{testharness:{
      demo:{ 'test.html':[blob,[null,{}]], 'shell.any.js':['unused',['demo/shell.any.js',{jsshell:true}]] },
      css:{'other.html':['unused',[null,{}]]}
    }}}));
    const out = path.join(root, 'results.json');
    const args = [path.join(import.meta.dir, 'run.mjs'), '--wpt', root, '--generated-globals', manifest,
      '--dirs', 'non-css', '--manifest-inventory', '--out', out, '--server', 'wptserve'];
    const list = spawnSync(process.execPath, [...args, '--list'], {encoding:'utf8'});
    expect(list.status).toBe(0);
    const listed=JSON.parse(fs.readFileSync(out));
    expect(listed.eligible.map(t=>[t.url,t.environment])).toEqual([['demo/test.html','window']]);
    expect(listed.inventoryOnly.map(t=>[t.url,t.environment])).toEqual([['demo/shell.any.js','jsshell']]);
    expect(listed.viewport).toEqual({width:800,height:600,deviceScaleFactor:1});
    fs.rmSync(out);
    const selected=path.join(root,'selected.json');
    fs.writeFileSync(selected,JSON.stringify(['demo/test.html','demo/shell.any.js']));
    const fail=spawnSync(process.execPath,[...args,'--tests',selected],{env:{...process.env,WPT_PYTHON:'/usr/bin/false'}});
    expect(fail.status).not.toBe(0);
    const partial=JSON.parse(fs.readFileSync(out+'.partial'));
    expect(partial.partial).toBe(true);
    expect(partial.results).toEqual([]);
    expect(partial.eligible).toEqual(listed.eligible);
    expect(partial.inventoryOnly).toEqual(listed.inventoryOnly);
    expect(partial.manifestSha256).toBe(listed.manifestSha256);
  } finally { fs.rmSync(root,{recursive:true,force:true}); }
});

test('historical skip review matches an exact URL and source blob and preserves transport exclusions', () => {
  const reviewed = JSON.parse(fs.readFileSync(new URL('./reviewed-gate-skips.json', import.meta.url)));
  const url = 'navigation-api/navigate-event/navigate-history-go-0.html';
  const source = 'navigate-history-go-0.html';
  const entry = [reviewed[url], [null, {}], [url + '?unreviewed', {}]];
  const manifest = {url_base:'/', items:{testharness:{
    'navigation-api':{'navigate-event':{[source]:entry}},
    fetch:{'h1-parsing':{'ordinary.html':['unused',[null,{}]]}}
  }}};
  const options = {dirs:['navigation-api','fetch'],http2:true,testdriver:true};
  const legacy = manifestInventory('.',manifest,options);
  expect(legacy.tests).toHaveLength(0);
  const measured = manifestInventory('.',manifest,{...options,measureReviewedSkips:true});
  expect(measured.tests.map(t=>t.url)).toEqual([url]);
  expect(measured.tests[0].gateSkipReason).toBe(legacy.info.inventoryOnly.find(t=>t.url===url).reason);
  expect(measured.info.inventoryOnly.find(t=>t.url.startsWith('fetch/')).reason).toBe('requires direct HTTP/1 origin bytes');
  entry[0]='unreviewed blob';
  expect(manifestInventory('.',manifest,{...options,measureReviewedSkips:true}).tests).toHaveLength(0);
});
