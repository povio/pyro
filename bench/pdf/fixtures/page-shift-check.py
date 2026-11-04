# python3 page-shift-check.py, in a dir with r-base/lp-fixed and r-head/lp-fixed from run.mjs: pixels that differ at 96 dpi after undoing a 1 CSS px page shift.
import subprocess,re,os,glob,json,sys
def pgm(f,pg):
    o=subprocess.run(['pdftoppm','-r','96','-f',str(pg),'-l',str(pg),'-gray',f],capture_output=True).stdout
    m=re.match(rb'P5\s+(\d+)\s+(\d+)\s+255\s',o); return int(m[1]),int(m[2]),o[m.end():]
res={}
for f in sorted(glob.glob('r-head/lp-fixed/*.pdf')):
    c=os.path.basename(f)[:-4]; g='r-base/lp-fixed/'+c+'.pdf'
    n=int(re.search(r'Pages:\s+(\d+)',subprocess.run(['pdfinfo',f],capture_output=True,text=True,errors='replace').stdout)[1])
    land=c=='bmd-report'; sx,sy=(1,0) if land else (0,1)
    bad=0;tot=0;badmax=0
    for pg in range(1,n+1):
        hw,hh,hd=pgm(f,pg); bw,bh,bd=pgm(g,pg)
        for y in range(0,bh):
            ry=bd[y*bw:y*bw+bw]; ty=y+sy
            if ty>=hh: continue
            rh=hd[ty*hw+sx:ty*hw+sx+bw]
            if ry!=rh:
                k=sum(1 for a,b in zip(ry,rh) if abs(a-b)>2); bad+=k
        tot+=bw*bh
    res[c]=[bad,tot]
    print(c,n,'pages differing px after undoing the shift',bad,'of',tot,flush=True)
json.dump(res,open('shiftonly.json','w'))
