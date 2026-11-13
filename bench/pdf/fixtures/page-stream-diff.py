# python3 page-stream-diff.py, in the same dir: per PDF, how each page stream changed (`cm` deltas, other numbers, structure).
import re,zlib,glob,os,json,collections,subprocess
def streams(p):
    d=open(p,'rb').read(); out=[]
    for m in re.finditer(rb'stream\r?\n(.*?)endstream',d,re.S):
        try: s=zlib.decompress(m.group(1))
        except: continue
        if b' cm' in s and (b' re' in s or b' m ' in s or b'BT' in s): out.append(s.decode('latin1'))
    return out
num=re.compile(r'-?\d*\.?\d+(?:e-?\d+)?')
res={}
for f in sorted(glob.glob('r-head/pyro-fixed/*.pdf')):
    c=os.path.basename(f)[:-4]; A=streams('r-base/pyro-fixed/'+c+'.pdf'); B=streams(f)
    other=0;cms=collections.Counter();bytes_same=True;nonmatch=0
    if len(A)!=len(B): res[c]={'streams':[len(A),len(B)]}; print(c,'stream count',len(A),len(B)); continue
    for a,b in zip(A,B):
        if a==b: continue
        ka=num.sub('#',a); kb=num.sub('#',b)
        if ka!=kb: nonmatch+=1; continue
        na=num.findall(a); nb=num.findall(b)
        # walk cm positions
        for ma,mb in zip(re.finditer(r'(\S+) (\S+) (\S+) (\S+) (\S+) (\S+) cm',a),re.finditer(r'(\S+) (\S+) (\S+) (\S+) (\S+) (\S+) cm',b)):
            ga=[float(x) for x in ma.groups() if re.fullmatch(num,x)]; gb=[float(x) for x in mb.groups() if re.fullmatch(num,x)]
            if len(ga)==6 and len(gb)==6:
                d=[round(q-p,3) for p,q in zip(ga,gb)]
                cms[tuple(d)]+=1
        # non-cm number changes
        sa=re.sub(r'\S+ \S+ \S+ \S+ \S+ \S+ cm','cm',a); sb=re.sub(r'\S+ \S+ \S+ \S+ \S+ \S+ cm','cm',b)
        for p,q in zip(num.findall(sa),num.findall(sb)):
            if p!=q: other+=1
    res[c]={'cm_deltas':{str(k):v for k,v in cms.items()},'other_number_changes':other,'structural':nonmatch}
    print(c,'cm delta',dict(cms),'other',other,'struct',nonmatch)
json.dump(res,open('streamdiff.json','w'),indent=0)
