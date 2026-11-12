# Writing tests with AI agents

The package ships a guide written for coding agents: `LLM.md`. It states the model (seed, then one run), every locator and step, the porting table from Playwright, how to read a failure, and what not to do, in a form an agent can follow. Point your agent at it before it writes or ports a test.

## Where the file is

`LLM.md` is part of the npm package, next to its README:

```text
node_modules/@povio/lightning-panda/LLM.md
```

It matches the installed version, so an agent reading it from `node_modules` gets the API your project actually has. The builders it describes are in `node_modules/@povio/lightning-panda/src/steps.ts` and pages and runs in `src/index.ts`; agents that read source can check details there.

## Pointing your agent at it

### Claude Code

Claude Code reads `CLAUDE.md` at the repository root and supports `@path` imports. Import the guide where your E2E tests live:

```markdown
<!-- e2e/CLAUDE.md -->
# E2E tests

Tests in this directory use @povio/lightning-panda's native driver.
Read and follow @../node_modules/@povio/lightning-panda/LLM.md before writing or changing a test.
```

### Cursor

Add a project rule that applies to your test files and references the guide:

```markdown
---
description: Writing E2E tests with @povio/lightning-panda
globs: e2e/**/*.test.ts
alwaysApply: false
---
Before writing or changing a test, read node_modules/@povio/lightning-panda/LLM.md and follow it.
```

Save it as `.cursor/rules/lightning-panda.mdc`.

### Codex and other agents

Agents that read `AGENTS.md` (Codex and others) take the same instruction:

```markdown
## E2E tests

Tests under e2e/ use @povio/lightning-panda. Before writing or changing one,
read node_modules/@povio/lightning-panda/LLM.md and follow it.
```

For an agent without file access, paste the contents of `LLM.md` into its context.

## A rules snippet

If you prefer a short project rule over the whole guide, or want to add your project's specifics on top of it, this summary follows `LLM.md`:

```markdown
## E2E tests (@povio/lightning-panda)

- A test seeds its data through the API, then makes ONE `page.run([...])`.
  Split into several runs only when plain code must run between them.
- Pages come from profiles: `await using page = await lp.page({ as: 'employee', route: '/path' })`.
  One profile per kind of user; sessions go in the profile's `localStorage`, never through the login UI.
- Locators: role + name first (`button('Save')`, `dialog().button('OK')`), then `label`,
  `placeholder`, `text`, then `testId`, then `css`. Strings are case-insensitive substrings;
  use `{ exact: true }` for whole matches.
- Assert with `check(loc).visible() / .text() / .count() / ...` inside the run.
  Assert requests with `check.request('POST', '/exact/path').status(200)` in the same run as the action.
- Use `read(as)` / `evaluate(expr, as)` only for values the test computes with; assert them with `expect(r.values.x)`.
- On a RunError, read the reason after the last colon and fix the locator or the step before it.
- Our profiles: <list them, with what each is signed in as>.
- Our API seed helpers: <where they live>.
```

## The do-nots

These are the mistakes agents make most often with a native driver. `LLM.md` lists them; repeat them in your rules if your agent drifts:

- **No waits.** No `sleep`, `waitForTimeout`, retry loops, `expect.poll`, or `waitForResponse`. Every step waits in the browser until it holds, and actions settle.
- **No try/catch around steps.** A failing step is information. Do not swallow it or retry it.
- **No UI login or UI seeding.** Use the profile for the session and the API for data.
- **No one-run-per-step.** It costs a round trip per step, and request checks see only their own run.
- **No shared pages.** Take a new page per test with `await using`.
- **No raised timeouts to make a test pass.** A timeout means the condition never held.
- **No rewriting of working Playwright suites.** To move a Playwright suite, change the imports to `@povio/lightning-panda/playwright` and keep the code. Rewrite into `page.run` only the tests whose speed matters.

## Reviewing what an agent wrote

A quick review catches most problems:

1. One `page.run` per test, seeded before it.
2. Every request the test cares about has a `check.request` in the same run as its action.
3. Locators are role-and-name or label based; `.first()` only where any match is fine.
4. No `evaluate` doing what an action or a check should do.
5. `await using` on every page.

Then run the test, and run it under load (for example `lightningpanda e2e -j` at twice your core count) to shake out races that a quiet machine hides.

See also [Porting from Playwright](/e2e/porting) for the mapping an agent uses when it ports a test, and [Debugging failures](/e2e/debugging) for the error messages it will meet.
