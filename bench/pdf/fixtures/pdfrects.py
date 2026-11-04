#!/usr/bin/env python3
# usage: pdfrects.py file.pdf -> JSON pages [{mediabox, rects:[[x0,y0,x1,y1],...]}]
import re,sys,zlib,json
d=open(sys.argv[1],'rb').read()
objs={}
for m in re.finditer(rb'(\d+) 0 obj\b(.*?)endobj',d,re.S):
    objs[int(m[1])]=m[2]
# object streams
for n,b in list(objs.items()):
    if b'/ObjStm' in b:
        s=re.search(rb'stream\r?\n(.*?)\r?\nendstream',b,re.S)
        if not s: continue
        try: raw=zlib.decompress(s[1])
        except Exception: continue
        N=int(re.search(rb'/N (\d+)',b)[1]); F=int(re.search(rb'/First (\d+)',b)[1])
        nums=list(map(int,raw[:F].split()))
        for i in range(N):
            oid,off=nums[2*i],nums[2*i+1]
            end=nums[2*i+3] if i+1<N else len(raw)-F
            objs[oid]=raw[F+off:F+end]
pages=[]
for n,b in sorted(objs.items()):
    if re.search(rb'/Type\s*/Page\b',b):
        mb=re.search(rb'/MediaBox\s*\[([^\]]*)\]',b)
        rects=[]
        an=re.search(rb'/Annots\s*\[([^\]]*)\]',b)
        anb=b
        if not an:
            ref=re.search(rb'/Annots\s+(\d+) 0 R',b)
            if ref: an=re.search(rb'\[([^\]]*)\]',objs[int(ref[1])])
        if an:
            for r in re.findall(rb'(\d+) 0 R',an[1]):
                o=objs.get(int(r),b'')
                rm=re.search(rb'/Rect\s*\[([^\]]*)\]',o)
                if rm: rects.append(list(map(float,rm[1].split())))
        pages.append({'obj':n,'mediabox':list(map(float,mb[1].split())) if mb else None,'rects':rects})
print(json.dumps(pages))
