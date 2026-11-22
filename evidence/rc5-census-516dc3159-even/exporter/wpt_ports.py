"""Canonicalize only encoded, WPT-owned URL authority ports using original wptserve config.
Use endpoint_map(result_file, page_url), canonical_name(name, map), and index(subtests, map).
For public wptrunner reports pass an empty map: their named URLs already use wptrunner ports.
No transformations of signature bytes or arbitrary hostnames, bare numbers or opaque strings.
"""
import collections, functools, hashlib,json,re
from pathlib import Path
DEFAULTS={'http':[8000,8001],'http-local':[8002],'http-public':[8003],'https':[8443,8444],'https-local':[8445],'https-public':[8446],'ws':[8888],'wss':[8889],'h2':[9000],'webtransport-h3':[11000],'dns':[8053]}
# An encodeURIComponent() URL (including nested encoding). Preserve casing and escape depth.
ENC=re.compile(r'(?P<scheme>https?|wss?)%(?P<depth>(?:25)*)3a%(?P=depth)2f%(?P=depth)2f(?P<host>(?:[a-z0-9_-]+\.)*(?:not-)?web-platform\.test)%(?P=depth)3a(?P<port>[0-9]+)(?![a-z0-9.-])',re.I)
def canonical_name(name,mapping):
    if not mapping or '%' not in name:return name
    def sub(m):
        if m.start() and name[m.start()-1].isascii() and (name[m.start()-1].isalnum() or name[m.start()-1] in '+.-'):
            # A nested URL may begin immediately after an escaped query delimiter.
            if not re.search(r'%(?:25)*(?:3d|26|2f|23|3f|20|22|27|3a|2c)$',name[:m.start()],re.I):return m[0]
        match=mapping.get((m['scheme'].lower(),int(m['port'])))
        if match is None:return m[0]
        return m[0][:m.start('port')-m.start()]+str(match)
    return ENC.sub(sub,name)
def index(names,mapping):
    counts=collections.Counter();seen=collections.defaultdict(set);out={}
    for name,status in names:
        canon=canonical_name(name,mapping);counts[canon]+=1;seen[canon].add(name)
        out[canon,counts[canon]]=(status,name)
    return out,{n:sorted(v) for n,v in seen.items() if len(v)>1}
@functools.lru_cache(None)
def config(log_path):
    log=Path(log_path)
    if not log.is_file():return {},{'log':str(log),'error':'missing-log'}
    matches=re.findall(r'^wptserve on http://web-platform\.test:(\d+) \(log (.+?/serve\.log)\)$',log.read_text(errors='replace'),re.M)
    if len(matches)!=1:return {},{'log':str(log),'error':'missing-or-ambiguous-server-announcement'}
    http,serverlog=matches[0];f=Path(serverlog).with_name('config.json');audit={'log':str(log),'config':str(f)}
    if not f.is_file():return {},audit|{'error':'missing-config'}
    raw=f.read_bytes();cfg=json.loads(raw);ports=cfg['ports'];audit.update(sha256=hashlib.sha256(raw).hexdigest(),ports=ports)
    if cfg['browser_host']!='web-platform.test' or cfg['server_host']!='127.0.0.1' or ports['http'][0]!=int(http):return {},audit|{'error':'config/log-origin-mismatch'}
    out={}
    for role,values in ports.items():
        if role not in DEFAULTS:continue
        if len(values)>len(DEFAULTS[role]):return {},audit|{'error':'unrecognized-port-slot'}
        for port,target in zip(values,DEFAULTS[role]):
            protocol={'h2':'https'}.get(role,role.split('-')[0]);key=(protocol,port)
            if key in out:return {},audit|{'error':'duplicate-port-for-scheme'}
            out[key]=target
    return out,audit
@functools.lru_cache(None)
def paths_for(result_path):
    f=Path(result_path);key=f.stem;d=f.parent
    # Split receipts are created by complete-interrupted.py. Its original and finish
    # files come from different server configs. Match source URL sets exactly.
    interruption=d.parent/'timing-interruptions'
    stem=f'{d.name}_{key}';orig=interruption/(stem+'.json')
    if orig.exists():
        doc=json.loads(f.read_bytes());parts=doc.get('inputParts')
        if parts:
            result={}
            for i,part in enumerate(parts):
                part=Path(part)
                log=interruption/(stem+'.resume.log') if i else d/(key+'.1.log')
                for row in json.loads(part.read_bytes())['results']:
                    u=row['url']
                    if u in result:raise ValueError(f'duplicate split provenance: {u}')
                    result[u]=log
            if set(result)!={r['url'] for r in doc['results']}:raise ValueError('incomplete split provenance: '+str(f))
            return None,result
    logs=list(d.glob(key+'.*.log'))+list(d.glob(key+'.log'))
    doc=json.loads(f.read_bytes())
    evidence=doc.get('inputPartProvenance')
    if evidence is not None:
        merged={row['url']:row for row in doc['results']};mapping={}
        if [r.get('file') for r in evidence]!=doc.get('inputParts'):return None,{}
        for r in evidence:
            part=Path(r['file']); selected=r['selectedUrls'];attempt=r.get('attempt')
            if len(selected)!=r['selectedCount'] or len(set(selected))!=len(selected) or set(mapping).intersection(selected):
                return None,{}
            if not selected:continue
            log=part.with_suffix('.log')
            # For integer attempts the partial and log must name the same numbered run.
            if isinstance(attempt,int) and (part.parent!=d or part.name!=key+f'.{attempt}.partial'):
                log=None
            try:
                raw=part.read_bytes()
                if hashlib.sha256(raw).hexdigest()!=r['sha256']:log=None
                else:
                    rows={x['url']:x for x in json.loads(raw)['results']}
                    if any(u not in rows or rows[u]!=merged.get(u) for u in selected):log=None
            except (OSError,ValueError,KeyError):log=None
            mapping.update({u:log for u in selected})
        if set(mapping)!=set(merged):return None,{}
        return None,mapping
    if len(logs)==1:return logs[0],{}
    completed=d/'completed.jsonl'
    if completed.exists():
        attempts={r.get('attempt') for line in completed.read_text().splitlines() if (r:=json.loads(line)).get('key')==key and r.get('rc')==0 and isinstance(r.get('attempt'),int)}
        if len(attempts)==1:return d/(key+f'.{attempts.pop()}.log'),{}
    return None,{}
def endpoint_map(result_path,url):
    default,byurl=paths_for(str(result_path));log=byurl.get(url,default)
    return config(str(log)) if log else ({},{'receipt':str(result_path),'error':'ambiguous-or-missing-logs'})
