// Computed role and label (WebDriver's Get Computed Role / Get Computed Label) and testdriver's accessibility
// properties of an element or an accessibility node, all read from Pyro's own accessibility tree over CDP
// (`Accessibility.getPartialAXTree` on the element), the way ChromeDriver reads Chrome's: the role string and the
// computed name of the element's own AX node. An ignored node (hidden, presentational, nothing semantic) reports the
// role the tree gives it (`none`) and no name.

// The element carrying the marker `testdriver-vendor.js` set for this call, as a remote object of the page's main
// frame's default context: found there, and in the same-origin frames inside it, depth first.
const find = (id) => `(() => {
  const sel = '[data-pyro-testdriver="${id}"]';
  const search = (doc) => {
    const el = doc.querySelector(sel);
    if (el) return el;
    for (const f of doc.querySelectorAll('iframe, frame')) {
      let d = null;
      try { d = f.contentDocument; } catch {}
      const found = d && search(d);
      if (found) return found;
    }
    return null;
  };
  return search(document);
})()`;

async function axNodes(ctx, id, fetchRelatives) {
  const session = await ctx.session();
  const found = await session.send('Runtime.evaluate', { expression: find(String(id).replace(/[^\w-]/g, '')) });
  const objectId = found.result?.objectId;
  if (!objectId) throw new Error('testdriver: no such element');
  try {
    const { nodes } = await session.send('Accessibility.getPartialAXTree', { objectId, fetchRelatives });
    if (!nodes?.length) throw new Error('testdriver: the element has no accessibility node');
    return nodes;
  } finally {
    await session.send('Runtime.releaseObject', { objectId }).catch(() => {});
  }
}

const axId = (n) => String(n.backendDOMNodeId ?? n.nodeId);
const ownRole = (n) => (n.role?.value ?? '');
const ownName = (n) => (n.name?.value ?? '');

// What the page of a window argument receives for an AX node: `{accessibilityId, role, label, parent, children,
// ...properties}`; each AX property as the string of its value (`checked: "true"`, `pressed: "mixed"`). `parent` and
// `children` are accessibility ids of the nearest unignored ancestor and the unignored descendants (ignored nodes
// stand for what they contain), the tree a platform accessibility API exposes.
function describe(nodes, self) {
  const byId = new Map(nodes.map((n) => [n.nodeId, n]));
  const out = { accessibilityId: axId(self), role: ownRole(self), label: ownName(self), children: [] };
  let p = byId.get(self.parentId);
  while (p && p.ignored) p = byId.get(p.parentId);
  if (p) out.parent = axId(p);
  const collect = (n) => {
    for (const c of (n.childIds ?? []).map((i) => byId.get(i)).filter(Boolean)) {
      if (c.ignored) collect(c);
      else out.children.push(axId(c));
    }
  };
  collect(self);
  for (const prop of self.properties ?? []) {
    const v = prop.value?.value;
    if (v !== undefined && !(prop.name in out)) out[prop.name] = typeof v === 'object' ? JSON.stringify(v) : String(v);
  }
  return out;
}

async function node(ctx, id) {
  return (await axNodes(ctx, id, false))[0];
}

export const commands = {
  'get_computed_role': async (ctx, [arg]) => ownRole(await node(ctx, arg.element)),
  'get_computed_label': async (ctx, [arg]) => ownName(await node(ctx, arg.element)),
  'get_accessibility_properties_for_element': async (ctx, [arg]) => {
    const nodes = await axNodes(ctx, arg.element, true);
    return describe(nodes, nodes[0]);
  },
  // An accessibility id from a previous answer: the node's backend DOM node id.
  'get_accessibility_properties_for_accessibility_node': async (ctx, [accId]) => {
    const session = await ctx.session();
    let nodes;
    try {
      ({ nodes } = await session.send('Accessibility.getPartialAXTree', { backendNodeId: Number(accId), fetchRelatives: true }));
    } catch {
      throw new Error('testdriver: no such accessibility node');
    }
    if (!nodes?.length) throw new Error('testdriver: no such accessibility node');
    return describe(nodes, nodes[0]);
  },
};
