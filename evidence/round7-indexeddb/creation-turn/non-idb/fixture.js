async () => {
  const frame = document.createElement('iframe');
  document.body.append(frame);
  const child = frame.contentWindow;
  const trace = [];
  const outer = new EventTarget();
  const nested = new EventTarget();
  const makeChildListener = child.Function('trace', 'outerWindow', 'outerTarget', `
    return function () {
      trace.push(['child callback', window !== outerWindow, parent === outerWindow, this === outerTarget]);
      outerWindow.dispatchEvent(new outerWindow.Event('nested-entry'));
      trace.push(['child resumes', window.parent === outerWindow]);
    };
  `);
  window.addEventListener('nested-entry', () => trace.push(['outer nested listener', window === document.defaultView]), {once:true});
  nested.addEventListener('inner', makeChildListener(trace, window, nested));
  outer.addEventListener('outer', () => {
    trace.push(['outer start', window === document.defaultView]);
    nested.dispatchEvent(new Event('inner'));
    trace.push(['outer resumes', frame.contentWindow === child]);
    queueMicrotask(() => trace.push(['microtask', window === document.defaultView]));
  });
  outer.addEventListener('outer', () => trace.push(['next listener', window === document.defaultView]));
  outer.dispatchEvent(new Event('outer'));
  trace.push(['dispatch returned']);
  await Promise.resolve();
  return trace;
}
