importScripts('model.js');
evaluateModel().then(result=>postMessage({result,secure:isSecureContext}),error=>postMessage({error:String(error)}));
