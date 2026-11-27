// WebDriver's virtual pressure sources (https://w3c.github.io/compute-pressure/#automation): "create / update / delete
// virtual pressure source" through the CDP methods Chrome has for them, Emulation.setPressureSourceOverrideEnabled,
// setPressureStateOverride and setPressureDataOverride. A source override belongs to the browser context (workers see
// it too), so the table of created sources is the context's.

const SOURCES = ['cpu'];
const STATES = ['nominal', 'fair', 'serious', 'critical'];

async function target(ctx, context) {
  const page = context ? await ctx.windowPage(context) : ctx.page;
  const session = await ctx.sessionOf(page);
  return { send: (method, params) => session.send(method, params), created: ctx.shared('pressure', () => new Set()) };
}

export const commands = {
  // `metadata.supported: false` makes the source one `observe()` rejects NotSupportedError for. An existing source of the
  // type is an error (WebDriver: invalid argument).
  'create_virtual_pressure_source': async (ctx, [source, metadata, context]) => {
    if (!SOURCES.includes(source)) throw new Error(`invalid argument: unknown pressure source type ${source}`);
    const { send, created } = await target(ctx, context);
    if (created.has(source)) throw new Error(`invalid argument: a virtual pressure source of type ${source} already exists`);
    const supported = metadata?.supported;
    if (supported !== undefined && typeof supported !== 'boolean') throw new Error('invalid argument: supported must be a boolean');
    await send('Emulation.setPressureSourceOverrideEnabled', { enabled: true, source, metadata: { available: supported ?? true } });
    created.add(source);
  },
  'update_virtual_pressure_source': async (ctx, [source, state, ownContributionEstimate, context]) => {
    if (!SOURCES.includes(source)) throw new Error(`invalid argument: unknown pressure source type ${source}`);
    if (!STATES.includes(state)) throw new Error(`invalid argument: unknown pressure state ${state}`);
    const estimate = ownContributionEstimate ?? undefined;
    if (estimate !== undefined && !(typeof estimate === 'number' && estimate >= 0 && estimate <= 1)) throw new Error('invalid argument: ownContributionEstimate must be a number from 0 to 1');
    const { send, created } = await target(ctx, context);
    if (!created.has(source)) throw new Error(`invalid argument: no virtual pressure source of type ${source}`);
    if (estimate === undefined) await send('Emulation.setPressureStateOverride', { source, state });
    else await send('Emulation.setPressureDataOverride', { source, state, ownContributionEstimate: estimate });
  },
  // Succeeds when there is no such source.
  'remove_virtual_pressure_source': async (ctx, [source, context]) => {
    if (!SOURCES.includes(source)) throw new Error(`invalid argument: unknown pressure source type ${source}`);
    const { send, created } = await target(ctx, context);
    if (!created.has(source)) return;
    await send('Emulation.setPressureSourceOverrideEnabled', { enabled: false, source });
    created.delete(source);
  },
};
