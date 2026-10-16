const { test } = require('node:test');
const assert = require('node:assert/strict');
const { artifactId } = require('../github-artifact-output.cjs');

test('accepts official actions/core multiline output with Windows newlines', () => {
  assert.equal(artifactId('artifact-id<<ghadelimiter_123\r\n987654\r\nghadelimiter_123\r\nartifact-url<<d\r\nhttps://example.invalid\r\nd\r\n'), '987654');
});
test('accepts the single-line runner output format', () => {
  assert.equal(artifactId('artifact-id=987654\n'), '987654');
});
test('rejects missing, duplicate, truncated and nonnumeric artifact IDs', () => {
  for (const text of ['', 'artifact-id=0\n', 'artifact-id=123\nartifact-id=456\n',
    'artifact-id<<d\n123\nwrong\n', 'artifact-id<<d\n123\n456\nd\n',
    'artifact-id=../123\n', 'artifact-id<<\n123\n\n']) {
    assert.throws(() => artifactId(text));
  }
});
