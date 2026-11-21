// Input rows are rewritten after submission to check tensor write snapshots.
async function evaluateModel() {
  const context = await navigator.ml.createContext({accelerated:false});
  const builder = new MLGraphBuilder(context);
  const f = shape => ({dataType:'float32',shape});
  const x = builder.input('x',f([1,2]));
  const weights = new Float32Array([1,-1,.5,2,1,-2]);
  const weight = builder.constant(f([2,3]),weights);
  weights.fill(99);
  const bias = builder.constant(f([3]),new Float32Array([-1,.5,2]));
  const product = builder.matmul(x,weight);
  const y = builder.relu(builder.add(product,bias));
  const graph = await builder.build({y});
  const inp = await context.createTensor({...f([1,2]),writable:true});
  const out = await context.createTensor({...f([1,3]),readable:true});
  const results=[];
  for(const row of [[2,3],[-1,2],[1,-2]]) {
    const storage=new Float32Array([41,...row,77]);
    context.writeTensor(inp,storage.subarray(1,3));storage.fill(17);
    context.dispatch(graph,{x:inp},{y:out});
    const dest=new Float32Array([31,0,0,0,39]);
    await context.readTensor(out,dest.subarray(1,4));
    results.push(Array.from(dest));
  }
  const response={results,accelerated:context.accelerated,shape:out.shape};
  graph.destroy();inp.destroy();out.destroy();context.destroy();
  return response;
}
