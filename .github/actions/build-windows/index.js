const { spawnSync } = require('node:child_process');
const path = require('node:path');
const root = process.env.GITHUB_WORKSPACE;
const env = { ...process.env, GATIT_NODE: process.execPath };
const result = spawnSync('pwsh', ['-NoProfile', '-NonInteractive', '-File',
  path.join(root, 'scripts/build-direct-windows.ps1')], { cwd: root, env, stdio: 'inherit' });
if (result.error) throw result.error;
process.exit(result.status ?? 1);
