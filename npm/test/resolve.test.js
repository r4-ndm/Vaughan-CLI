'use strict';
// Offline unit tests for the platform/checksum logic. No network, no release needed:
//   node test/resolve.test.js
const assert = require('assert');
const { resolveTarget, parseShasums, downloadUrl, TARGETS } = require('../lib/resolve');

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

check('linux x64 maps to the x86_64 tarball', () => {
  const r = resolveTarget('linux', 'x64');
  assert.deepStrictEqual(r, { asset: 'vaughan-linux-x86_64.tar.gz', key: 'linux-x64' });
});

check('linux arm64 maps to the aarch64 tarball', () => {
  assert.strictEqual(resolveTarget('linux', 'arm64').asset, 'vaughan-linux-aarch64.tar.gz');
});

check('darwin arm64 maps to macos-aarch64', () => {
  assert.strictEqual(resolveTarget('darwin', 'arm64').asset, 'vaughan-macos-aarch64.tar.gz');
});

check('unsupported platform returns an actionable error, not a throw', () => {
  const r = resolveTarget('win32', 'x64');
  assert.ok(r.error, 'expected an error object');
  assert.ok(/unsupported platform win32-x64/.test(r.error.message), r.error.message);
  assert.ok(/VAUGHAN_BIN/.test(r.error.message), 'error must tell the user the way out');
});

check('intel macOS is explicitly unsupported rather than silently mapped', () => {
  assert.ok(resolveTarget('darwin', 'x64').error);
});

check('shasums parse the real release format (two spaces)', () => {
  const text =
    'a'.repeat(64) + '  vaughan-linux-aarch64.tar.gz\n' +
    'b'.repeat(64) + '  vaughan-linux-x86_64.tar.gz\n' +
    'c'.repeat(64) + '  vaughan-macos-aarch64.tar.gz\n';
  assert.strictEqual(parseShasums(text, 'vaughan-linux-x86_64.tar.gz'), 'b'.repeat(64));
});

check('shasums parse the binary-mode asterisk form', () => {
  assert.strictEqual(parseShasums('d'.repeat(64) + ' *vaughan-linux-x86_64.tar.gz', 'vaughan-linux-x86_64.tar.gz'), 'd'.repeat(64));
});

check('a missing asset returns null instead of guessing', () => {
  assert.strictEqual(parseShasums('e'.repeat(64) + '  other.tar.gz', 'vaughan-linux-x86_64.tar.gz'), null);
});

check('a malformed sums file returns null rather than a truncated hash', () => {
  assert.strictEqual(parseShasums('not-a-hash  vaughan-linux-x86_64.tar.gz', 'vaughan-linux-x86_64.tar.gz'), null);
});

check('download URL is pinned and tagged', () => {
  assert.strictEqual(
    downloadUrl('0.1.0', 'vaughan-linux-x86_64.tar.gz'),
    'https://github.com/r4-ndm/Vaughan-CLI/releases/download/v0.1.0/vaughan-linux-x86_64.tar.gz'
  );
});

check('download URL refuses a non-version string', () => {
  assert.throws(() => downloadUrl('../../etc/passwd', 'vaughan-linux-x86_64.tar.gz'));
});

check('every target maps to a distinct asset', () => {
  const assets = Object.values(TARGETS);
  assert.strictEqual(new Set(assets).size, assets.length);
});

console.log(failures === 0 ? '\nall resolve tests passed' : `\n${failures} test(s) failed`);
process.exit(failures === 0 ? 0 : 1);
