// Compile-only (`bun run test:types`): the README's manual tool loop against
// the Anthropic SDK's types, and the shapes the converters promise.
import Anthropic from '@anthropic-ai/sdk';
import { INSTRUCTIONS, createBrowserTools, toAnthropicContent, toAnthropicTools, toOpenAITools, type ToolResult } from '../../src/agent/index.js';

export async function readmeLoop(): Promise<void> {
  const client = new Anthropic();
  const browser = createBrowserTools({ allowedOrigins: ['https://shop.example.com'] });
  const tools = toAnthropicTools(browser.tools, { strict: true });
  const messages: Anthropic.MessageParam[] = [{ role: 'user', content: 'Put the cheapest kettle in the basket.' }];
  for (;;) {
    const reply = await client.messages.create({
      model: 'claude-opus-5-5', max_tokens: 16000, output_config: { effort: 'medium' },
      cache_control: { type: 'ephemeral' }, system: INSTRUCTIONS, tools, messages,
    });
    messages.push({ role: 'assistant', content: reply.content });
    const calls = reply.content.filter((b) => b.type === 'tool_use');
    if (!calls.length) break;
    const results = [];
    for (const call of calls) {
      const r = await browser.execute(call.name, call.input);
      results.push({ type: 'tool_result' as const, tool_use_id: call.id, content: toAnthropicContent(r.content), is_error: r.isError });
    }
    messages.push({ role: 'user', content: results });
  }
  await browser.close();
}

// Tool definitions are Claude tools as they are.
const asClaude: Anthropic.Tool[] = toAnthropicTools(createBrowserTools().tools);
// OpenAI's function tools: { type: 'function', function: { name, description, parameters, strict } }.
const asOpenAI: { type: 'function'; function: { name: string; parameters: object; strict: boolean } }[] = toOpenAITools(createBrowserTools().tools);
// A result's content blocks fit a Claude tool_result.
declare const result: ToolResult;
const block: Anthropic.ToolResultBlockParam = { type: 'tool_result', tool_use_id: 'x', content: toAnthropicContent(result.content), is_error: result.isError };
// @ts-expect-error execute takes the input object, not a JSON string's name pair
createBrowserTools().execute({ name: 'browser_snapshot' });
export { asClaude, asOpenAI, block };
