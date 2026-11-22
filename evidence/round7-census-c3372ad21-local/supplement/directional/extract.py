"""Export occurrence and strict-page pass-set differences from a verified census."""
from argparse import ArgumentParser
from collections import Counter, defaultdict
from contextlib import contextmanager
import gzip
import hashlib
import json
from pathlib import Path

def require(condition, message):
    if not condition:raise ValueError(message)

def sha(path):
    with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()

def read(path):
    return json.loads(path.read_bytes())

def rows(path):
    with gzip.open(path,'rb') as f:yield from map(json.loads,f)

def save(path,data):
    path.write_text(json.dumps(data,indent=1,sort_keys=True,ensure_ascii=True)+'\n')

@contextmanager
def compressed(path):
    with path.open('wb') as f,gzip.GzipFile(filename='',fileobj=f,mode='wb',compresslevel=5,mtime=0) as stream:
        yield lambda row:stream.write(json.dumps(row,separators=(',',':'),ensure_ascii=True).encode('ascii')+b'\n')

def passing(cell):
    return cell is not None and cell[0]=='Pass'

def diagnostics(folder,provenance,key,wanted,expected):
    plain=folder/(key+'.json');packed=folder/(key+'.json.gz')
    if plain.exists():
        raw=plain.read_bytes()
    else:
        with gzip.open(packed,'rb') as f:raw=f.read()
    digest=hashlib.sha256(raw).hexdigest()
    require(digest==provenance[key][expected],'source receipt digest mismatch: '+key)
    r=json.loads(raw)
    out={}
    for page in r['results']:
        u=page['url']
        if u not in wanted:continue
        subtests=page['subtests'];n=sum(t['status']=='Pass' for t in subtests)
        pg=[page['status'],page.get('harnessStatus'),bool(subtests) and page['status']=='complete' and page.get('harnessStatus')=='OK' and n==len(subtests),n,len(subtests)]
        require(pg==wanted[u],'published page differs from raw: '+u)
        counter=Counter();nonpass=[]
        for test in subtests:
            name=test['name'];counter[name]+=1
            if test['status']!='Pass':
                nonpass.append([name,counter[name],test['status'],test.get('msg')])
        out[u]=dict(page=pg,receiptKey=key,receiptSHA256=digest,error=page.get('error'),
                    harnessMessage=page.get('harnessMessage'),nonpassingSubtests=nonpass)
    require(set(out)==set(wanted),'source receipt did not provide every changed page: '+key)
    return out

def main():
    a=ArgumentParser(description=__doc__)
    a.add_argument('--matched',type=Path,required=True)
    a.add_argument('--current-raw',type=Path,required=True)
    a.add_argument('--previous-raw',type=Path,required=True)
    a.add_argument('--chrome-raw',type=Path,required=True)
    a.add_argument('--out',type=Path,required=True)
    args=a.parse_args();origin=args.matched;out=args.out;out.mkdir(exist_ok=True,parents=True)
    checksums={n:d for d,n in [l.split('  ',1) for l in (origin/'SHA256SUMS').read_text().splitlines()]}
    def checked(name):
        p=origin/name;require(sha(p)==checksums[name],'matched input digest mismatch '+name);return p
    summary=read(checked('summary.json'));counts=summary['counts']
    require(summary['requestedScopeComplete'] and not summary['historicalDemonstration'],'incomplete or demonstration input')
    changed={};sources=[defaultdict(dict) for _ in range(3)]
    for u,head,chrome,prev,keys,err in rows(checked('pages.jsonl.gz')):
        if head[2]==prev[2]:continue
        direction='gain' if head[2] else 'loss'
        changed[u]={'url':u,'direction':direction,'previouslyPassingNamesNowAbsent':[],'newlyObservedPassingNames':[]}
        for role,(pg,key) in enumerate(zip([head,chrome,prev],keys)):
            require(pg is not None and key is not None,'missing matched page '+u)
            sources[role][key.removeprefix('chrome/')][u]=pg
    kinds=Counter(p['direction'] for p in changed.values());cp=counts['currentPrevious']
    require(kinds['loss']==cp['secondOnlyPage'] and kinds['gain']==cp['firstOnlyPage'],'incorrect number of strict-page changes')
    sides=Counter();current_only=out/'lp-only-vs-chrome.jsonl.gz';stable_only=out/'chrome-only-vs-lp.jsonl.gz'
    with compressed(current_only) as lp,compressed(stable_only) as chrome:
        for u,n,i,h,b,p in rows(checked('named.jsonl.gz')):
            if passing(h)!=passing(b):
                side='lpOnly' if passing(h) else 'chromeOnly';sides[side]+=1
                (lp if passing(h) else chrome)([u,n,i,h,b,p])
            if u in changed:
                if passing(p) and h is None:changed[u]['previouslyPassingNamesNowAbsent'].append([n,i,p,b])
                if passing(h) and p is None:changed[u]['newlyObservedPassingNames'].append([n,i,h,b])
    require(sides['lpOnly']==counts['currentChrome']['firstOnly'] and sides['chromeOnly']==counts['currentChrome']['secondOnly'],'incorrect number of directional names')
    for mapping,(role,folder,proof,field) in zip(sources,[
        ('current',args.current_raw,'current-provenance.json','receiptSHA256'),
        ('signedChrome',args.chrome_raw,'chrome-provenance.json','rawSHA256'),
        ('previousLP',args.previous_raw,'previous-provenance.json','receiptSHA256')]):
        provenance=read(checked(proof))
        for key,us in mapping.items():
            for u,diagnostic in diagnostics(folder,provenance,key,us,field).items():
                changed[u][role]=diagnostic
    for kind,file in [('gain','strict-gains.jsonl.gz'),('loss','strict-losses.jsonl.gz')]:
        with compressed(out/file) as emit:
            for u in sorted(changed):
                if changed[u]['direction']==kind:emit(changed[u])
    save(out/'summary.json',dict(matchedSnapshot=summary['snapshot'],matchedSHA256SUMS=sha(origin/'SHA256SUMS'),
        head=summary['head'],tree=summary['tree'],wpt=summary['wpt'],chrome=summary['chrome'],
        previous=summary['previous'],planSHA256=summary['planSHA256'],matchedPages=counts['current']['pages'],
        completeShards=summary['shardsComplete'],lpOnly=sides['lpOnly'],chromeOnly=sides['chromeOnly'],
        strictGains=kinds['gain'],strictLosses=kinds['loss'],previousFull=counts['previous']['strictPages'],
        currentFull=counts['current']['strictPages'],chromeFull=counts['chrome']['strictPages']))
    files=sorted(p for p in out.iterdir() if p.is_file() and p.name!='SHA256SUMS')
    (out/'SHA256SUMS').write_text(''.join(sha(p)+'  '+p.name+'\n' for p in files))
    print((out/'summary.json').read_text())

if __name__=='__main__':main()
