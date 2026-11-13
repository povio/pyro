import type { FullConfig } from '../../../src/wright/index';

// As Playwright's docs write one: the options come from a project's resolved `use`.
export default async function globalSetup(config: FullConfig) {
  const { baseURL } = config.projects[0].use;
  const app = config.projects.find((p) => p.name === 'app')!;
  const r = await fetch(`${baseURL}/ping`);
  console.log(`GLOBAL SETUP ping ${r.status}, projects ${config.projects.map((p) => p.name).join(',')}, app depends on ${app.dependencies.join(',')}, app storageState ${app.use.storageState === process.env.DEPS_STATE}, workers ${config.workers}, shard ${JSON.stringify(config.shard)}`);
}
