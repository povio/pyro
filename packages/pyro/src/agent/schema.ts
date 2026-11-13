// JSON Schemas of the browser tools: the subset both providers' strict modes
// take, a validator for tool inputs, and the conversion to OpenAI's form.
//
// The schemas are written in the form Claude's `strict: true` wants: every
// object closed (`additionalProperties: false`), an optional parameter is one
// left out of `required`, and no keyword outside the supported subset (no
// `minimum`, `minLength`, `pattern`, `format` or `default`: ranges and
// defaults live in the descriptions and the executor checks them).
// OpenAI's strict mode wants every property in `required` and an optional one
// as a union with null; `toOpenAISchema` makes that form, and the executor
// treats a null argument as one left out.

export type JsonType = 'string' | 'number' | 'integer' | 'boolean' | 'array' | 'object' | 'null';

export type JsonSchema = {
  type?: JsonType | JsonType[];
  description?: string;
  title?: string;
  enum?: (string | number | boolean | null)[];
  const?: string | number | boolean | null;
  properties?: Record<string, JsonSchema>;
  required?: string[];
  additionalProperties?: false;
  items?: JsonSchema;
  anyOf?: JsonSchema[];
};

/** A tool's input: a closed object. */
export type ObjectSchema = JsonSchema & {
  type: 'object';
  properties: Record<string, JsonSchema>;
  required: string[];
  additionalProperties: false;
};

/** Builders that keep the tool table short and every object closed. */
export const s = {
  string: (description: string): JsonSchema => ({ type: 'string', description }),
  number: (description: string): JsonSchema => ({ type: 'number', description }),
  integer: (description: string): JsonSchema => ({ type: 'integer', description }),
  boolean: (description: string): JsonSchema => ({ type: 'boolean', description }),
  enum: <T extends string>(values: readonly T[], description: string): JsonSchema => ({ type: 'string', enum: [...values], description }),
  array: (items: JsonSchema, description: string): JsonSchema => ({ type: 'array', items, description }),
  object: (properties: Record<string, JsonSchema>, required: string[] = []): ObjectSchema => ({
    type: 'object',
    properties,
    required,
    additionalProperties: false,
  }),
};

const KEYWORDS = new Set(['type', 'description', 'title', 'enum', 'const', 'properties', 'required', 'additionalProperties', 'items', 'anyOf']);

/**
 * What keeps `schema` from a provider's strict mode ([] when nothing does):
 * a closed object at the root and at every level, `required` naming only
 * listed properties, and no keyword outside the shared subset. `openai`
 * also wants every property required (an optional one as a union with null).
 */
export function strictProblems(schema: JsonSchema, provider: 'anthropic' | 'openai'): string[] {
  const problems: string[] = [];
  if (schema.type !== 'object' || schema.anyOf) problems.push('root: must be a plain object schema (type: "object", no anyOf)');
  walk(schema, 'root', (node, path) => {
    for (const key of Object.keys(node)) if (!KEYWORDS.has(key)) problems.push(`${path}: unsupported keyword "${key}"`);
    for (const v of node.enum ?? []) if (v !== null && typeof v === 'object') problems.push(`${path}: enum values must be primitives`);
    const types = Array.isArray(node.type) ? node.type : node.type ? [node.type] : [];
    if (Array.isArray(node.type) && !(types.length === 2 && types.includes('null'))) problems.push(`${path}: a type list must be [T, "null"]`);
    if (!node.type && !node.anyOf && node.const === undefined && !node.enum) problems.push(`${path}: no type`);
    if (types.includes('array') && !node.items) problems.push(`${path}: array without items`);
    if (!types.includes('object')) return;
    if (node.additionalProperties !== false) problems.push(`${path}: additionalProperties must be false`);
    if (!node.properties) problems.push(`${path}: object without properties`);
    if (!Array.isArray(node.required)) problems.push(`${path}: required must be listed`);
    const keys = Object.keys(node.properties ?? {});
    for (const r of node.required ?? []) if (!keys.includes(r)) problems.push(`${path}: required "${r}" is not a property`);
    if (provider === 'openai') {
      for (const k of keys) if (!node.required?.includes(k)) problems.push(`${path}.${k}: OpenAI strict mode wants every property in required (optional as a union with null)`);
    }
  });
  return problems;
}

/** Calls `fn` on every schema node, depth first. */
function walk(node: JsonSchema, path: string, fn: (node: JsonSchema, path: string) => void): void {
  fn(node, path);
  for (const [k, v] of Object.entries(node.properties ?? {})) walk(v, `${path}.${k}`, fn);
  if (node.items) walk(node.items, `${path}[]`, fn);
  node.anyOf?.forEach((v, i) => walk(v, `${path}|${i}`, fn));
}

/** Counts across strict schemas: Claude limits a request to 24 optional and 16 union-typed parameters. */
export function strictCost(schema: JsonSchema): { optional: number; unions: number } {
  let optional = 0;
  let unions = 0;
  walk(schema, 'root', (node) => {
    if (Array.isArray(node.type) || node.anyOf) unions++;
    const keys = Object.keys(node.properties ?? {});
    optional += keys.filter((k) => !node.required?.includes(k)).length;
  });
  return { optional, unions };
}

/** OpenAI's strict form of `schema`: every property required, the optional ones nullable. */
export function toOpenAISchema(schema: JsonSchema): JsonSchema {
  const out: JsonSchema = { ...schema };
  if (schema.items) out.items = toOpenAISchema(schema.items);
  if (schema.anyOf) out.anyOf = schema.anyOf.map(toOpenAISchema);
  if (schema.properties) {
    const required = new Set(schema.required ?? []);
    out.properties = Object.fromEntries(
      Object.entries(schema.properties).map(([k, v]) => [k, required.has(k) ? toOpenAISchema(v) : nullable(toOpenAISchema(v))]),
    );
    out.required = Object.keys(schema.properties);
  }
  return out;
}

function nullable(schema: JsonSchema): JsonSchema {
  if (typeof schema.type === 'string' && schema.type !== 'object' && !schema.anyOf) {
    const out: JsonSchema = { ...schema, type: [schema.type, 'null'] };
    if (schema.enum) out.enum = [...schema.enum, null];
    return out;
  }
  const { description, ...rest } = schema;
  return { ...(description ? { description } : {}), anyOf: [rest, { type: 'null' }] };
}

/** `value` without its null properties, at every level (OpenAI's strict mode sends null for a left-out optional). */
export function dropNulls<T>(value: T): T {
  if (Array.isArray(value)) return value.map(dropNulls) as T;
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).filter(([, v]) => v !== null).map(([k, v]) => [k, dropNulls(v)])) as T;
  }
  return value;
}

/** The first way `value` breaks `schema` (as a message for the model), or null. */
export function validate(schema: JsonSchema, value: unknown, path = 'input'): string | null {
  if (schema.anyOf) {
    const errors = schema.anyOf.map((s) => validate(s, value, path));
    return errors.includes(null) ? null : errors[0];
  }
  const types = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : [];
  if (types.length && !types.some((t) => isType(t, value))) {
    return `${path} must be ${types.join(' or ')}, got ${value === null ? 'null' : Array.isArray(value) ? 'an array' : typeof value}`;
  }
  if (schema.enum && !schema.enum.includes(value as never)) return `${path} must be one of ${schema.enum.map((v) => JSON.stringify(v)).join(', ')}`;
  if (schema.const !== undefined && value !== schema.const) return `${path} must be ${JSON.stringify(schema.const)}`;
  if (Array.isArray(value) && schema.items) {
    for (let i = 0; i < value.length; i++) {
      const e = validate(schema.items, value[i], `${path}[${i}]`);
      if (e) return e;
    }
  }
  if (value && typeof value === 'object' && !Array.isArray(value) && schema.properties) {
    const obj = value as Record<string, unknown>;
    for (const r of schema.required ?? []) if (obj[r] === undefined) return `${path}.${r} is required`;
    for (const [k, v] of Object.entries(obj)) {
      const prop = schema.properties[k];
      if (!prop) {
        if (schema.additionalProperties === false) return `${path}.${k} is not a parameter of this tool (parameters: ${Object.keys(schema.properties).join(', ') || 'none'})`;
        continue;
      }
      if (v === undefined) continue;
      const e = validate(prop, v, `${path}.${k}`);
      if (e) return e;
    }
  }
  return null;
}

function isType(t: JsonType, v: unknown): boolean {
  switch (t) {
    case 'string': return typeof v === 'string';
    case 'number': return typeof v === 'number' && Number.isFinite(v);
    case 'integer': return Number.isInteger(v);
    case 'boolean': return typeof v === 'boolean';
    case 'array': return Array.isArray(v);
    case 'object': return !!v && typeof v === 'object' && !Array.isArray(v);
    case 'null': return v === null;
  }
}
