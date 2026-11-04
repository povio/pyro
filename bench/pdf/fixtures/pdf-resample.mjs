// bun pdf-resample.mjs <chrome.pdf> <base.pdf> <head.pdf> [k]: per page, error against Chrome at 150 dpi as poppler rasterizes it, and box-averaged from 150k dpi.
import { execFileSync } from 'node:child_process'; import { createRequire } from 'node:module';
const require = createRequire(new URL('../package.json', import.meta.url)); const { PNG } = require('pngjs'); const pm = require('pixelmatch'); const pixelmatch = pm.default ?? pm;
const pgm=(f,page,dpi)=>{const o=execFileSync('pdftoppm',['-r',dpi,'-f',page,'-l',page,'-gray',f],{maxBuffer:2000000000,stdio:['ignore','pipe','ignore']});let i=0,tok=[];while(tok.length<4){while(o[i]<=32)i++;let s=i;while(o[i]>32)i++;tok.push(o.slice(s,i).toString())}i++;return{w:+tok[1],h:+tok[2],d:o.subarray(i)}};
const down=(im,k)=>{const w=Math.floor(im.w/k),h=Math.floor(im.h/k),d=new Uint8Array(w*h);for(let y=0;y<h;y++)for(let x=0;x<w;x++){let s=0;for(let j=0;j<k;j++){const r=(y*k+j)*im.w+x*k;for(let i=0;i<k;i++)s+=im.d[r+i]}d[y*w+x]=Math.round(s/(k*k))}return{w,h,d}};
const cmp=(a,b)=>{const w=Math.min(a.w,b.w),h=Math.min(a.h,b.h);let s=0;const A=new Uint8ClampedArray(w*h*4),B=new Uint8ClampedArray(w*h*4);for(let y=0;y<h;y++)for(let x=0;x<w;x++){const p=a.d[y*a.w+x],q=b.d[y*b.w+x];s+=Math.abs(p-q);const i=(y*w+x)*4;A[i]=A[i+1]=A[i+2]=p;A[i+3]=255;B[i]=B[i+1]=B[i+2]=q;B[i+3]=255}return [Math.round(s/1000),pixelmatch(A,B,null,w,h,{threshold:.2})]};
const [c,m,t,kk=4,from=1,to=99]=process.argv.slice(2);const n=+/Pages:\s+(\d+)/.exec(execFileSync('pdfinfo',[c],{encoding:'utf8',stdio:['ignore','pipe','ignore']}))[1];
let tot=[0,0,0,0,0,0,0,0];
for(let p=+from;p<=Math.min(n,+to);p++){const k=+kk;const C=pgm(c,p,150),M=pgm(m,p,150),T=pgm(t,p,150);const r1=cmp(C,M),r2=cmp(C,T);const Ca=down(pgm(c,p,150*k),k),Ma=down(pgm(m,p,150*k),k),Ta=down(pgm(t,p,150*k),k);const r3=cmp(Ca,Ma),r4=cmp(Ca,Ta);
console.log(`p${p} 150dpi SAD/px main ${r1} head ${r2} | box-avg from ${150*k} main ${r3} head ${r4}`);[...r1,...r2,...r3,...r4].forEach((v,i)=>tot[i]+=v);}
console.log('total',`150dpi main ${tot[0]},${tot[1]} head ${tot[2]},${tot[3]} | box-avg main ${tot[4]},${tot[5]} head ${tot[6]},${tot[7]}`);process.exit(0);
