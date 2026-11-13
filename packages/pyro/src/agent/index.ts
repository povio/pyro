// `@povio/pyro/agent`: Pyro as the browser of an LLM agent, for developers who
// run their own agent loop (the Claude API's tool use, or any provider's).
//
//   const browser = createBrowserTools();                    // launches Pyro on first use
//   const tools = toAnthropicTools(browser.tools, { strict: true });
//   // … the model calls a tool …
//   const result = await browser.execute(call.name, call.input);
//   const content = toAnthropicContent(result.content);      // tool_result content blocks
//   await browser.close();
//
// The same tools back the MCP server (`pyro mcp`). See AGENTS.md for how a
// model should use them.
import { errorResult, quote, type ToolResult } from './content.js';
import { dropNulls, strictCost, toOpenAISchema, validate, type JsonSchema, type ObjectSchema } from './schema.js';
import { BrowserAgentSession, withTimeout, type BrowserAgentOptions } from './session.js';
import { CAPABILITIES, TOOLS, ToolInputError, type BrowserTool, type Capability, type ToolDefinition } from './tools.js';

export { BrowserAgentSession, Tab, type BrowserAgentOptions } from './session.js';
export { CAPABILITIES, TOOLS, type BrowserTool, type Capability, type ToolAnnotations, type ToolDefinition } from './tools.js';
export {
  toAnthropicContent,
  toolResultText,
  UNTRUSTED,
  type AnthropicImageBlock,
  type AnthropicTextBlock,
  type ImageContent,
  type ResourceLinkContent,
  type TextContent,
  type ToolContent,
  type ToolResult,
} from './content.js';
export { strictProblems, toOpenAISchema, validate, type JsonSchema, type ObjectSchema } from './schema.js';
export { OriginPolicy } from './origins.js';
export { ProfileStore, type Identity, type ProfileInfo } from './profiles.js';
export { INSTRUCTIONS } from './instructions.js';

/**
 * The tool groups `caps` asks for: a list of groups to keep (`core` always
 * stays), or groups prefixed with `-` or `no-` to switch off from all of them
 * (`'-evaluate,-files'`). Undefined: every group.
 */
export function resolveCaps(caps?: string | string[]): Set<Capability> {
  const entries = (Array.isArray(caps) ? caps : (caps ?? '').split(/[,\s]+/)).map((c) => c.trim()).filter(Boolean);
  const off = entries.filter((c) => /^(-|no-)/.test(c)).map((c) => c.replace(/^(-|no-)/, ''));
  const on = entries.filter((c) => !/^(-|no-)/.test(c));
  for (const c of [...on, ...off]) {
    if (!(CAPABILITIES as readonly string[]).includes(c)) throw new Error(`unknown tool group ${quote(c)}: groups are ${CAPABILITIES.join(', ')}`);
  }
  const set = new Set<Capability>(on.length ? ['core', ...(on as Capability[])] : CAPABILITIES);
  for (const c of off) if (c !== 'core') set.delete(c as Capability);
  return set;
}

/** The tools of `caps` (stored identities only when profiles may be written). */
export function selectTools(caps: Set<Capability>, o: { isolated?: boolean } = {}): BrowserTool[] {
  return TOOLS.filter((t) => caps.has(t.capability) && !(t.capability === 'profiles' && o.isolated)).map((t) => {
    // Without the files group nothing is written to disk: no screenshot file name (nor includeImage: false, which needs one).
    if (t.name !== 'browser_take_screenshot' || caps.has('files')) return t;
    const { filename: _filename, includeImage: _includeImage, ...properties } = t.inputSchema.properties;
    return { ...t, inputSchema: { ...t.inputSchema, properties } };
  });
}

/** A tool's definition as data (no `execute`). */
export function toolDefinition(t: BrowserTool): ToolDefinition {
  return {
    name: t.name,
    title: t.title,
    description: t.description,
    inputSchema: t.inputSchema,
    ...(t.outputSchema ? { outputSchema: t.outputSchema } : {}),
    annotations: { title: t.title, ...t.annotations },
  };
}

/**
 * Runs `tool` with `input` on `session`: checks the input against the
 * schema (a null argument counts as left out), refuses page tools while a
 * dialog is open, and returns every failure as an `isError` result. Calls run
 * one at a time per session. `signal` cancels a call (its result is then a
 * cancellation; an action already sent to the page still completes).
 */
export async function executeTool(tool: BrowserTool, rawInput: unknown, session: BrowserAgentSession, signal?: AbortSignal): Promise<ToolResult> {
  const input = dropNulls(rawInput ?? {});
  const problem = validate(tool.inputSchema, input);
  if (problem) return errorResult(`Invalid input for ${tool.name}: ${problem}.`);
  if (signal?.aborted) return errorResult('Cancelled.');
  const run = session.run(async (): Promise<ToolResult> => {
    if (signal?.aborted) return errorResult('Cancelled.');
    const modal = tool.modalSafe ? undefined : session.modalTab();
    if (modal?.dialog) {
      return errorResult(
        `A ${modal.dialog.type()} dialog is open on tab [${session.tabs.indexOf(modal)}]: ${quote(modal.dialog.message())}. It holds the page: answer it with browser_handle_dialog first.`,
      );
    }
    try {
      return await tool.execute(input, session, signal);
    } catch (e) {
      return errorResult(failure(tool, e));
    }
  });
  if (!signal) return run;
  return Promise.race([
    run,
    new Promise<ToolResult>((resolve) => signal.addEventListener('abort', () => resolve(errorResult('Cancelled.')), { once: true })),
  ]);
}

function failure(tool: BrowserTool, e: unknown): string {
  if (e instanceof ToolInputError) return e.message;
  const msg = e instanceof Error ? e.message : String(e);
  const hint = /Timeout \d+ms exceeded/.test(msg)
    ? '\nThe element may be hidden, disabled, covered or gone. Take a new snapshot (browser_snapshot) to see the page as it is now.'
    : '';
  return `${tool.name} failed: ${msg}${hint}`;
}

export type BrowserToolsOptions = BrowserAgentOptions & {
  /** Tool groups (see `resolveCaps`): e.g. `'-evaluate'` or `['core', 'tabs']`. Default: all. */
  caps?: string | string[];
  /** A session to use instead of a new one. */
  session?: BrowserAgentSession;
};

export type BrowserTools = {
  /** The tool definitions, provider-neutral (MCP's shape), in a fixed order. */
  tools: ToolDefinition[];
  session: BrowserAgentSession;
  /** Runs a tool by name; failures come back as results with `isError`, it does not throw. */
  execute(name: string, input: unknown, options?: { signal?: AbortSignal }): Promise<ToolResult>;
  /** Closes the browser (a launched one exits). */
  close(): Promise<void>;
};

/** The browser tools on a session of their own (the browser starts with the first tool call). */
export function createBrowserTools(options: BrowserToolsOptions = {}): BrowserTools {
  const session = options.session ?? new BrowserAgentSession(options);
  const selected = selectTools(resolveCaps(options.caps), { isolated: session.profiles.readOnly });
  const byName = new Map(selected.map((t) => [t.name, t]));
  return {
    tools: selected.map(toolDefinition),
    session,
    async execute(name, input, o = {}) {
      const tool = byName.get(name);
      if (!tool) return errorResult(`Unknown tool ${quote(name)}. The browser tools are: ${[...byName.keys()].join(', ')}.`);
      return executeTool(tool, input, session, o.signal);
    },
    close: () => withTimeout(session.close(), 10_000).catch(() => undefined),
  };
}

/** A tool as the Claude API takes it. */
export type AnthropicToolDefinition = { name: string; description: string; input_schema: ObjectSchema; strict?: boolean };

/** Claude's per-request limits for `strict: true` tools. */
const CLAUDE_STRICT = { tools: 20, optional: 24, unions: 16 };

/**
 * The tools in the Claude API's shape (`{ name, description, input_schema }`).
 * `strict: true` marks tools strict (inputs then always match the schema) in
 * order, as many as Claude's per-request limits allow (20 strict tools, 24
 * optional and 16 union-typed parameters across them); the rest go without
 * it. A list of names marks those only (within the same limits).
 */
export function toAnthropicTools(tools: ToolDefinition[], options: { strict?: boolean | string[] } = {}): AnthropicToolDefinition[] {
  const left = { ...CLAUDE_STRICT };
  return tools.map((t) => {
    const out: AnthropicToolDefinition = { name: t.name, description: t.description, input_schema: structuredClone(t.inputSchema) };
    const wanted = options.strict === true || (Array.isArray(options.strict) && options.strict.includes(t.name));
    if (!wanted) return out;
    const cost = strictCost(t.inputSchema);
    if (left.tools < 1 || cost.optional > left.optional || cost.unions > left.unions) return out;
    left.tools--;
    left.optional -= cost.optional;
    left.unions -= cost.unions;
    return { ...out, strict: true };
  });
}

/** A tool as OpenAI's Chat Completions API takes it. */
export type OpenAIToolDefinition = { type: 'function'; function: { name: string; description: string; parameters: JsonSchema; strict: boolean } };

/**
 * The tools in OpenAI's function shape, in strict mode: every property
 * required, the optional ones nullable (`execute` treats null as left out).
 */
export function toOpenAITools(tools: ToolDefinition[]): OpenAIToolDefinition[] {
  return tools.map((t) => ({ type: 'function', function: { name: t.name, description: t.description, parameters: toOpenAISchema(t.inputSchema), strict: true } }));
}
