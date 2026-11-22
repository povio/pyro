#!/usr/bin/env python3 -I
# Same-origin controls for inline module identity vs base URL / referrer.
# usage: controls.py label=/path/to/pyro [label=/path ...]
import hashlib, json, re, subprocess, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PAGE = """<!doctype html><base href="/one/#auth">
<script type="importmap">{"imports":{"x":"/x-top.js"},"scopes":{"/one/#auth":{"x":"/x-frag.js"},"/ordinary.js#inline-module-authored":{"x":"/x-ext.js"}}}</script>
<body>
<script type=module>import {v} from 'x'; document.body.setAttribute('p1', import.meta.url + ' ' + v)</script>
<script type=module>document.body.setAttribute('p2', import.meta.url + ' ' + import.meta.resolve('x'));
document.querySelector('base').href='/two/#kept';
for (const id of ['d1','d2']) {const s=document.createElement('script');s.type='module';
s.textContent="document.body.setAttribute('"+id+"', import.meta.url + ' ' + import.meta.resolve('./r.js') + ' ' + import.meta.resolve('x'))";document.body.appendChild(s)}</script>
<script type=module src="/ordinary.js#inline-module-authored"></script>
<script type=module src="/plain.js"></script>
</body>"""
JS = {
    "/ordinary.js": "import {v} from 'x'; document.body.setAttribute('external', import.meta.url + ' ' + v)",
    "/plain.js": "document.body.setAttribute('plain', import.meta.url)",
    "/x-top.js": "export const v='top'", "/x-frag.js": "export const v='frag'", "/x-ext.js": "export const v='ext'",
}

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split('#')[0].split('?')[0]
        body = JS.get(path)
        self.send_response(200)
        self.send_header('Content-Type', 'text/javascript' if body is not None else 'text/html')
        self.end_headers()
        self.wfile.write((body if body is not None else PAGE).encode())
    def log_message(self, *a): pass

server = ThreadingHTTPServer(('127.0.0.1', 0), H)
threading.Thread(target=server.serve_forever, daemon=True).start()
origin = f'http://127.0.0.1:{server.server_port}'
expected = {
    'p1': f'{origin}/one/#auth frag',
    'p2': f'{origin}/one/#auth {origin}/x-frag.js',
    'd1': f'{origin}/two/#kept {origin}/two/r.js {origin}/x-top.js',
    'd2': f'{origin}/two/#kept {origin}/two/r.js {origin}/x-top.js',
    'external': f'{origin}/ordinary.js#inline-module-authored ext',
    'plain': f'{origin}/plain.js',
}
out = {'origin': origin, 'expected': expected, 'runs': {}}
for arg in sys.argv[1:]:
    label, binary = arg.split('=', 1)
    r = subprocess.run([binary, 'fetch', '--wait-ms', '500', origin + '/index.html'], capture_output=True, text=True, timeout=60)
    body = re.search(r'<body[^>]*>', r.stdout)
    attrs = dict(re.findall(r'(\w+)="([^"]*)"', body.group() if body else ''))
    attrs = {k: v.replace('&amp;', '&') for k, v in attrs.items()}
    out['runs'][label] = {
        'binary': hashlib.sha256(open(binary, 'rb').read()).hexdigest(),
        'observed': attrs,
        'match': {k: attrs.get(k) == v for k, v in expected.items()},
    }
print(json.dumps(out, indent=1))
