// Use the pinned official GitHub upload action and SignPath action, including
// GitHub's server-generated origin evidence. Never submit local arbitrary files
// through the generic SignPath REST endpoint, which lacks trusted-build proof.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const { artifactId } = require('./github-artifact-output.cjs');

function run(entry, inputs, output) {
  const env = { ...process.env, GITHUB_OUTPUT: output };
  for (const key of Object.keys(env)) if (key.startsWith('INPUT_')) delete env[key];
  for (const [key, value] of Object.entries(inputs)) env[`INPUT_${key.toUpperCase()}`] = String(value);
  const result = spawnSync(process.execPath, [entry], { env, stdio: 'inherit' });
  if (result.error || result.status !== 0) throw new Error('Official signing/upload action failed');
}

if (process.env.GITHUB_ACTIONS !== 'true' || process.env.GITHUB_REF !== 'refs/heads/main' ||
    process.env.GITHUB_EVENT_NAME === 'pull_request' || process.env.RUNNER_ENVIRONMENT !== 'github-hosted') {
  throw new Error('SignPath release signing requires GitHub-hosted main builds');
}
for (const key of ['SIGNPATH_API_TOKEN', 'SIGNPATH_ORGANIZATION_ID', 'SIGNPATH_PROJECT_SLUG',
  'SIGNPATH_SIGNING_POLICY_SLUG', 'SIGNPATH_ARTIFACT_CONFIGURATION_SLUG', 'SIGNPATH_GITHUB_TOKEN',
  'GATIT_VERSION', 'GITHUB_WORKSPACE', 'RUNNER_TEMP']) {
  if (!process.env[key]) throw new Error(`Missing ${key}`);
}
const file = path.resolve(process.argv[2]);
const root = process.env.GITHUB_WORKSPACE;
const work = fs.mkdtempSync(path.join(process.env.RUNNER_TEMP, 'gatit-signpath-'));
const input = path.join(work, 'input');
const output = path.join(work, 'signed');
fs.mkdirSync(input);
fs.copyFileSync(file, path.join(input, 'Gatit-signing-input.exe'));
try {
  const uploadOutput = path.join(work, 'upload-output');
  run(path.join(root, '.build/upload-action/dist/upload/index.js'), {
    name: `signpath-${crypto.randomUUID()}`, path: input,
    'if-no-files-found': 'error', 'retention-days': '7', 'compression-level': '0',
    overwrite: 'false', 'include-hidden-files': 'false'
  }, uploadOutput);
  const id = artifactId(fs.readFileSync(uploadOutput, 'utf8'));
  run(path.join(root, '.build/signpath-action/index.js'), {
    'connector-url': 'https://pipelineconnector.connectors.signpath.io/GitHubActions/GitHubCom',
    'api-token': process.env.SIGNPATH_API_TOKEN,
    'organization-id': process.env.SIGNPATH_ORGANIZATION_ID,
    'project-slug': process.env.SIGNPATH_PROJECT_SLUG,
    'signing-policy-slug': process.env.SIGNPATH_SIGNING_POLICY_SLUG,
    'artifact-configuration-slug': process.env.SIGNPATH_ARTIFACT_CONFIGURATION_SLUG,
    'github-artifact-id': id, 'github-token': process.env.SIGNPATH_GITHUB_TOKEN,
    'wait-for-completion': 'true', 'wait-for-completion-timeout-in-seconds': '3600',
    'service-unavailable-timeout-in-seconds': '600', 'download-signed-artifact-timeout-in-seconds': '300',
    'output-artifact-directory': output, 'skip-decompress': 'false',
    parameters: `version: ${JSON.stringify(process.env.GATIT_VERSION)}`
  }, path.join(work, 'signpath-output'));
  const entries = fs.readdirSync(output);
  if (entries.length !== 1 || entries[0] !== 'Gatit-signing-input.exe') {
    throw new Error('Unexpected signed output files');
  }
  const signed = path.join(output, entries[0]);
  if (!fs.lstatSync(signed).isFile()) throw new Error('Signed result is not a regular file');
  // Check that the executable content is unchanged except Authenticode fields.
  const check = spawnSync('python', [path.join(root, 'scripts/verify-pe-content.py'), file, signed], { stdio: 'inherit' });
  if (check.status !== 0) throw new Error('Signing changed executable content');
  fs.copyFileSync(signed, file);
} finally { fs.rmSync(work, { recursive: true, force: true }); }
