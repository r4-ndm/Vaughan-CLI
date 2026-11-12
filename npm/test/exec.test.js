'use strict';
// Proves the wrapper forwards the right argv to the real binary without a download:
// VAUGHAN_BIN points at a fake `vaughan` that prints its arguments.
//   node test/exec.test.js
const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const wrapper = path.join(__dirname, '..', 'bin', 'vaughan-mcp.js');
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'vaughan-exec-'));
const fake = path.join(dir, 'vaughan');
fs.writeFileSync(fake, '#!/bin/sh\nprintf "%s\\n" "$@"\n', { mode: 0o755 });

function run(args, env) {
  const res = spawnSync(process.execPath, [wrapper, ...args], {
    encoding: 'utf8',
    env: { ...process.env, VAUGHAN_BIN: fake, ...env },
  });
  return { out: (res.stdout || '').trim(), err: (res.stderr || '').trim(), status: res.status };
}

let failures = 0;
function check(name, fn) {
  try {
    fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures += 1;
    console.log(`FAIL ${name}: ${e.message}`);
  }
}

check('always prefixes the mcp subcommand', () => {
  const { out } = run([], { VAUGHAN_PROFILE: '' });
  assert.strictEqual(out.split('\n')[0], 'mcp');
});

check('passes client args through unchanged', () => {
  const { out } = run(['--profile', 'sentient'], { VAUGHAN_PROFILE: '' });
  assert.deepStrictEqual(out.split('\n'), ['mcp', '--profile', 'sentient']);
});

check('VAUGHAN_PROFILE supplies a profile when the client did not', () => {
  const { out } = run([], { VAUGHAN_PROFILE: 'sentient' });
  assert.deepStrictEqual(out.split('\n'), ['mcp', '--profile', 'sentient']);
});

check('an explicit --profile wins over the environment', () => {
  const { out } = run(['--profile', 'default'], { VAUGHAN_PROFILE: 'sentient' });
  assert.deepStrictEqual(out.split('\n'), ['mcp', '--profile', 'default']);
});

check('a missing VAUGHAN_BIN target fails with an actionable message', () => {
  const { err, status } = run([], { VAUGHAN_BIN: path.join(dir, 'nope') });
  assert.notStrictEqual(status, 0, 'expected a non-zero exit');
  assert.ok(/does not exist/.test(err), err);
});

check('child exit code is propagated', () => {
  const failing = path.join(dir, 'vaughan-fail');
  fs.writeFileSync(failing, '#!/bin/sh\nexit 7\n', { mode: 0o755 });
  const { status } = run([], { VAUGHAN_BIN: failing });
  assert.strictEqual(status, 7);
});

fs.rmSync(dir, { recursive: true, force: true });
console.log(failures === 0 ? '\nall exec tests passed' : `\n${failures} test(s) failed`);
process.exit(failures === 0 ? 0 : 1);
