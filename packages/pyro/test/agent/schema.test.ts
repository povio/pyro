// The tool definitions without a browser: strict-mode schemas for Claude and
// OpenAI, the provider converters, result conversion, and the instructions.
import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  INSTRUCTIONS,
  TOOLS,
  createBrowserTools,
  strictProblems,
  toAnthropicContent,
  toAnthropicTools,
  toOpenAISchema,
  toOpenAITools,
  toolResultText,
  validate,
  type JsonSchema,
} from '../../src/agent/index';
import { fence } from '../../src/agent/content';
import { strictCost } from '../../src/agent/schema';

const all = createBrowserTools().tools;

/** Playwright MCP's tool names that Pyro implements under the same name and shape. */
const PLAYWRIGHT_MCP = [
  'browser_navigate', 'browser_navigate_back', 'browser_snapshot', 'browser_click', 'browser_type', 'browser_fill_form',
  'browser_select_option', 'browser_hover', 'browser_drag', 'browser_press_key', 'browser_wait_for', 'browser_evaluate',
  'browser_take_screenshot', 'browser_pdf_save', 'browser_console_messages', 'browser_network_requests', 'browser_tabs',
  'browser_handle_dialog', 'browser_file_upload', 'browser_resize', 'browser_close',
];

describe('tool definitions', () => {
  test('the Playwright MCP tools plus browser_get_text and browser_profiles, each fully described', () => {
    expect(all.map((t) => t.name).sort()).toEqual([...PLAYWRIGHT_MCP, 'browser_get_text', 'browser_profiles'].sort());
    for (const t of all) {
      expect(t.name).toMatch(/^browser_[a-z_]+$/);
      expect(t.title.length).toBeGreaterThan(3);
      expect(t.description.length).toBeGreaterThan(40);
      for (const k of ['readOnlyHint', 'destructiveHint', 'idempotentHint', 'openWorldHint'] as const) expect(typeof t.annotations[k]).toBe('boolean');
      expect(t.annotations.title).toBe(t.title);
      // Every parameter is described for the model.
      const walk = (s: JsonSchema, path: string) => {
        for (const [k, v] of Object.entries(s.properties ?? {})) {
          if (!v.description) throw new Error(`${path}.${k} has no description`);
          walk(v, `${path}.${k}`);
        }
        if (s.items) walk(s.items, `${path}[]`);
      };
      walk(t.inputSchema, t.name);
    }
  });

  test('every input schema passes the strict-mode checks of Claude, and of OpenAI once converted', () => {
    for (const t of all) {
      expect({ tool: t.name, problems: strictProblems(t.inputSchema, 'anthropic') }).toEqual({ tool: t.name, problems: [] });
      const openai = toOpenAISchema(t.inputSchema);
      expect({ tool: t.name, problems: strictProblems(openai, 'openai') }).toEqual({ tool: t.name, problems: [] });
    }
  });

  test('the strict checker catches what the providers reject', () => {
    const ok: JsonSchema = { type: 'object', properties: { a: { type: 'string', description: 'x' } }, required: [], additionalProperties: false };
    expect(strictProblems(ok, 'anthropic')).toEqual([]);
    expect(strictProblems(ok, 'openai')).toEqual(['root.a: OpenAI strict mode wants every property in required (optional as a union with null)']);
    expect(strictProblems({ ...ok, additionalProperties: undefined }, 'anthropic')).toContain('root: additionalProperties must be false');
    expect(strictProblems({ type: 'object', properties: { n: { type: 'integer', minimum: 1 } as JsonSchema }, required: ['n'], additionalProperties: false }, 'anthropic')).toContain('root.n: unsupported keyword "minimum"');
    expect(strictProblems({ type: 'object', properties: { o: { type: 'object', properties: {}, required: [] } }, required: ['o'], additionalProperties: false }, 'anthropic')).toContain('root.o: additionalProperties must be false');
    expect(strictProblems({ type: 'array', items: { type: 'string' } } as JsonSchema, 'anthropic')[0]).toContain('root: must be a plain object schema');
    expect(strictProblems({ ...ok, required: ['b'] }, 'anthropic')).toContain('root: required "b" is not a property');
  });

  test('OpenAI form: every property required, the optional ones nullable', () => {
    const click = all.find((t) => t.name === 'browser_click')!;
    const s = toOpenAISchema(click.inputSchema);
    expect(s.required).toEqual(['element', 'ref', 'button', 'doubleClick', 'modifiers']);
    expect(s.properties!.ref.type).toBe('string');
    expect(s.properties!.button).toMatchObject({ type: ['string', 'null'], enum: ['left', 'right', 'middle', null] });
    expect(s.properties!.modifiers.type).toEqual(['array', 'null']);
    const form = toOpenAISchema(all.find((t) => t.name === 'browser_fill_form')!.inputSchema);
    expect(form.properties!.fields.items!.additionalProperties).toBe(false);
    const tools = toOpenAITools(all);
    expect(tools[0]).toMatchObject({ type: 'function', function: { name: 'browser_navigate', strict: true } });
  });

  test('Claude form: strict on as many tools as the per-request limits allow, in order', () => {
    const tools = toAnthropicTools(all, { strict: true });
    expect(tools.map((t) => t.name)).toEqual(all.map((t) => t.name));
    const strict = tools.filter((t) => t.strict);
    const cost = strict.map((t) => strictCost(t.input_schema)).reduce((a, c) => ({ optional: a.optional + c.optional, unions: a.unions + c.unions }), { optional: 0, unions: 0 });
    expect(strict.length).toBeLessThanOrEqual(20);
    expect(cost.optional).toBeLessThanOrEqual(24);
    expect(cost.unions).toBeLessThanOrEqual(16);
    for (const name of ['browser_navigate', 'browser_snapshot', 'browser_click', 'browser_type', 'browser_fill_form', 'browser_handle_dialog']) {
      expect(tools.find((t) => t.name === name)!.strict).toBe(true);
    }
    expect(toAnthropicTools(all).some((t) => 'strict' in t)).toBe(false);
    expect(toAnthropicTools(all, { strict: ['browser_click'] }).filter((t) => t.strict).map((t) => t.name)).toEqual(['browser_click']);
    // Copies: changing one does not change the tool table.
    tools[0].input_schema.properties.url.description = 'changed';
    expect(TOOLS[0].inputSchema.properties.url.description).not.toBe('changed');
  });

  test('the validator', () => {
    const schema = all.find((t) => t.name === 'browser_fill_form')!.inputSchema;
    expect(validate(schema, { fields: [{ name: 'a', type: 'textbox', ref: 'e1', value: 'x' }] })).toBeNull();
    expect(validate(schema, { fields: [{ name: 'a', type: 'select', ref: 'e1', value: 'x' }] })).toContain('input.fields[0].type must be one of');
    expect(validate(schema, { fields: [{ name: 'a', type: 'textbox', ref: 'e1' }] })).toBe('input.fields[0].value is required');
    expect(validate(schema, { fields: 'a' })).toBe('input.fields must be array, got string');
  });
});

describe('results', () => {
  test('MCP content as Claude tool_result blocks', () => {
    const blocks = toAnthropicContent([
      { type: 'text', text: 'hi' },
      { type: 'image', data: 'AAAA', mimeType: 'image/png' },
      { type: 'resource_link', uri: 'file:///tmp/out/a%20b.pdf', name: 'a b.pdf', mimeType: 'application/pdf' },
    ]);
    expect(blocks).toEqual([
      { type: 'text', text: 'hi' },
      { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'AAAA' } },
      { type: 'text', text: 'File: /tmp/out/a b.pdf (application/pdf)' },
    ]);
    expect(toolResultText({ content: [{ type: 'text', text: 'a' }, { type: 'image', data: 'x', mimeType: 'image/jpeg' }] })).toBe('a\n\n[image/jpeg image left out: this API takes text results only]');
  });

  test('a fence the text inside cannot close', () => {
    expect(fence('plain')).toBe('```\nplain\n```');
    expect(fence('a ``` b ````` c', 'yaml')).toBe('``````yaml\na ``` b ````` c\n``````');
  });
});

test('AGENTS.md carries the MCP instructions word for word', () => {
  const doc = readFileSync(join(import.meta.dir, '../../AGENTS.md'), 'utf8');
  const m = /<!-- instructions:start -->\n([^]*?)\n<!-- instructions:end -->/.exec(doc);
  expect(m?.[1]).toBe(INSTRUCTIONS);
});
