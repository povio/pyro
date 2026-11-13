// Files the API writes where the caller says (`path` options, `saveAs`):
// missing parent directories are made first, as Playwright does.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';

export function writeFileMakingDirs(path: string, data: string | Uint8Array): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, data);
}
