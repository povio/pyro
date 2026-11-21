const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
if (!process.env.WPT_CHECKOUT) throw new Error('Set WPT_CHECKOUT to the pinned WPT checkout');
const wpt = path.join(process.env.WPT_CHECKOUT, 'webnn/conformance_tests');
const vectors = [];
for (const name of ['relu','matmul','gemm']) {
  const text = fs.readFileSync(path.join(wpt,name+'.https.any.js'),'utf8');
  const body = text.slice(text.indexOf('const '+name+'Tests = [')+('const '+name+'Tests = [').length,text.indexOf('\n];'));
  const units = body.split(/\n(?=  \{\n    'name':)/).filter(s=>s.trim());
  for (const unit of units) {
    const title = unit.match(/'name':\s*'([^']+)'/)[1];
    if (!/float32/i.test(title) || /large|8192|stress|overflow/i.test(title))continue;
    let test; try {test = vm.runInNewContext('('+unit.slice(0,unit.lastIndexOf('}')+1)+'\n)',Object.create(null));} catch(e){console.error(name,title,unit.slice(-220));throw e;}
    const inputs = Object.values(test.graph.inputs);
    if(inputs.some(i=>i.descriptor.dataType!=='float32'||i.descriptor.shape.reduce((a,b)=>a*b,1)>1024))continue;
    if(name==='matmul' && inputs.some(i=>i.descriptor.shape.length!==2))continue;
    vectors.push({source:name+'.https.any.js',...test});
  }
}
fs.writeFileSync(path.join(__dirname,'../probes/cases/vectors.js'),'const cpuVectorFixtures = '+JSON.stringify(vectors)+';\n');
console.log(vectors.reduce((out,t)=>(out[t.source]=(out[t.source]||0)+1,out),{}));
