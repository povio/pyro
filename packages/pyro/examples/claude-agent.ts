// Claude drives the Pyro browser through the Anthropic TypeScript SDK's beta
// tool runner: every browser tool becomes a `betaTool()` whose `run` calls
// Pyro, and the runner loops (request, tool calls, tool results) until
// Claude answers.
//
//   bun add -d @anthropic-ai/sdk
//   ANTHROPIC_API_KEY=… bun examples/claude-agent.ts "What is on https://example.com?"
//
// The browser starts with the first tool call (the bundled browser, or
// $PYRO_BIN / a running server at $PYRO_WS) and exits when the run ends.
import Anthropic from '@anthropic-ai/sdk';
import { betaTool } from '@anthropic-ai/sdk/helpers/beta/json-schema';
import { ToolError } from '@anthropic-ai/sdk/lib/tools/ToolError';
import { INSTRUCTIONS, createBrowserTools, toAnthropicContent, toAnthropicTools } from '@povio/pyro/agent';

const task = process.argv.slice(2).join(' ') || 'Open https://example.com and tell me what the page is for.';

const client = new Anthropic();
// No JavaScript tool for this agent: it browses like a user.
const browser = createBrowserTools({ caps: '-evaluate' });

// The tool list is the same on every request, in the same order, so it stays
// in the prompt cache. `strict` makes Claude's inputs match the schemas (on as
// many tools as the API's strict-mode limits allow); Pyro checks every input
// too, since betaTool() does not validate JSON Schema inputs at run time.
const tools = toAnthropicTools(browser.tools, { strict: true }).map((t) =>
  betaTool({
    name: t.name,
    description: t.description,
    inputSchema: t.input_schema,
    ...(t.strict ? { strict: true } : {}),
    run: async (input, context) => {
      const result = await browser.execute(t.name, input, { signal: context?.signal ?? undefined });
      // Text and screenshots as tool_result content blocks; a failure goes back with is_error.
      const content = toAnthropicContent(result.content);
      if (result.isError) throw new ToolError(content);
      return content;
    },
  }),
);

try {
  const message = await client.beta.messages.toolRunner({
    model: 'claude-opus-5-5',
    max_tokens: 16000,
    // Opus 5.5 always thinks (adaptively); effort is the control, and its default is medium.
    output_config: { effort: 'medium' },
    // Caches the prefix of every request: the tools, the system prompt and the conversation so far.
    cache_control: { type: 'ephemeral' },
    // A request the model declines is retried server-side on the recommended fallback model.
    betas: ['server-side-fallback-2026-07-01'],
    fallbacks: 'default',
    system: `${INSTRUCTIONS}\n\nUse the browser_* tools to do the user's task, then answer from what the pages showed.`,
    tools,
    messages: [{ role: 'user', content: task }],
    max_iterations: 40,
    // The runner sends each turn on a message thread (beta, not zero-data-retention
    // eligible); `thread: false` sends the whole conversation every turn instead.
  });
  if (message.stop_reason === 'refusal') {
    console.error(`Declined${message.stop_details?.category ? ` (${message.stop_details.category})` : ''}: ${message.stop_details?.explanation ?? 'no explanation'}`);
    process.exitCode = 1;
  } else if (message.stop_reason === 'max_tokens') {
    console.error('The answer was cut off at max_tokens.');
    process.exitCode = 1;
  } else if (message.stop_reason === 'tool_use') {
    console.error('Stopped after max_iterations with tool calls still pending.');
    process.exitCode = 1;
  }
  for (const block of message.content) if (block.type === 'text') console.log(block.text);
} finally {
  await browser.close();
}

// The same loop by hand, without the beta runner (one tool_result per call,
// all of a turn's results in one user message):
//
//   const messages: Anthropic.MessageParam[] = [{ role: 'user', content: task }];
//   const plain = toAnthropicTools(browser.tools, { strict: true });
//   for (;;) {
//     const reply = await client.messages.create({
//       model: 'claude-opus-5-5', max_tokens: 16000, output_config: { effort: 'medium' },
//       cache_control: { type: 'ephemeral' }, system: INSTRUCTIONS, tools: plain, messages,
//     });
//     messages.push({ role: 'assistant', content: reply.content });
//     const calls = reply.content.filter((b) => b.type === 'tool_use');
//     if (reply.stop_reason !== 'tool_use' || !calls.length) break;
//     const results = await Promise.all(calls.map(async (call) => {
//       const r = await browser.execute(call.name, call.input);
//       return { type: 'tool_result' as const, tool_use_id: call.id, content: toAnthropicContent(r.content), is_error: r.isError };
//     }));
//     messages.push({ role: 'user', content: results });
//   }
