'use strict';
// Offline unit tests for the `vaughan` human launcher and bundled `vaughan-mcp` bin:
//   node test/launcher.test.js
const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const {
  RELEASES_URL,
  PLATFORM_PACKAGES,
  resolvePlatformPackage,
  resolveBinary,
} = require('../launcher/lib/resolve');

const launcherRoot = path.join(__dirname, '..', 'launcher');
const vaughanBin = path.join(launcherRoot, 'bin', 'vaughan.js');
const vaughanMcpBin = path.join(launcherRoot, 'bin', 'vaughan-mcp.js');

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'vaughan-launcher-test-'));
const fake = path.join(tmpRoot, 'vaughan');
fs.writeFileSync(
  fake,
  [
    '#!/bin/sh',
    'if [ "${1:-}" = "--version" ] || [ "${1:-}" = "-V" ]; then',
    '  echo "vaughan 0.2.6"',
    '  exit 0',
    'fi',
    'for a in "$@"; do printf "%s\\n" "$a"; done',
  ].join('\n') + '\n',
  { mode: 0o755 }
);

function runScript(scriptPath, args, extraEnv = {}) {
  const env = { ...process.env };
  delete env.VAUGHAN_BIN;
  delete env.VAUGHAN_PROFILE;
  delete env.VAUGHAN_TEST_PLATFORM;
  delete env.VAUGHAN_TEST_ARCH;
  Object.assign(env, extraEnv);

  const res = spawnSync(process.execPath, [scriptPath, ...args], {
    encoding: 'utf8',
    env,
  });
  return {
    out: (res.stdout || '').trim(),
    err: (res.stderr || '').trim(),
    status: res.status,
  };
}

let failures = 0;
function check(name, fn) {
  try {
    fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures += 1;
    console.log(`FAIL ${name}: ${e.stack || e.message}`);
  }
}

check('platform to npm package mapping matches release targets', () => {
  assert.strictEqual(resolvePlatformPackage('linux', 'x64').pkg, 'vaughan-linux-x64');
  assert.strictEqual(resolvePlatformPackage('linux', 'arm64').pkg, 'vaughan-linux-arm64');
  assert.strictEqual(resolvePlatformPackage('darwin', 'arm64').pkg, 'vaughan-darwin-arm64');
  assert.strictEqual(Object.keys(PLATFORM_PACKAGES).length, 3);
});

check('unsupported platform returns UNSUPPORTED_PLATFORM error with Releases URL', () => {
  for (const [plat, arch] of [
    ['win32', 'x64'],
    ['darwin', 'x64'],
  ]) {
    const res = resolvePlatformPackage(plat, arch);
    assert.ok(res.error, `expected error for ${plat}-${arch}`);
    assert.strictEqual(res.error.code, 'UNSUPPORTED_PLATFORM');
    assert.ok(res.error.message.includes('UNSUPPORTED_PLATFORM'), res.error.message);
    assert.ok(res.error.message.includes(RELEASES_URL), res.error.message);
  }
});

check('vaughan and vaughan-mcp bins print UNSUPPORTED_PLATFORM + releases URL on unsupported platform', () => {
  for (const binPath of [vaughanBin, vaughanMcpBin]) {
    const { err, status } = runScript(binPath, ['--version'], {
      VAUGHAN_TEST_PLATFORM: 'win32',
      VAUGHAN_TEST_ARCH: 'x64',
    });
    assert.notStrictEqual(status, 0, 'expected non-zero exit');
    assert.ok(err.includes('UNSUPPORTED_PLATFORM'), err);
    assert.ok(err.includes(RELEASES_URL), err);
  }
});

check('resolveBinary locates platform package via require.resolve / NODE_PATH', () => {
  const pkgRoot = path.join(tmpRoot, 'nm', 'node_modules', 'vaughan-linux-x64');
  fs.mkdirSync(path.join(pkgRoot, 'bin'), { recursive: true });
  fs.writeFileSync(
    path.join(pkgRoot, 'package.json'),
    JSON.stringify({ name: 'vaughan-linux-x64', version: '0.2.6' })
  );
  const pkgBin = path.join(pkgRoot, 'bin', 'vaughan');
  fs.copyFileSync(fake, pkgBin);
  fs.chmodSync(pkgBin, 0o755);

  const nodePath = path.join(tmpRoot, 'nm', 'node_modules');
  const vRes = runScript(vaughanBin, ['balance', '--json'], {
    NODE_PATH: nodePath,
    VAUGHAN_TEST_PLATFORM: 'linux',
    VAUGHAN_TEST_ARCH: 'x64',
  });
  assert.strictEqual(vRes.status, 0, vRes.err);
  assert.deepStrictEqual(vRes.out.split('\n'), ['balance', '--json']);

  const mRes = runScript(vaughanMcpBin, ['--version'], {
    NODE_PATH: nodePath,
    VAUGHAN_TEST_PLATFORM: 'linux',
    VAUGHAN_TEST_ARCH: 'x64',
  });
  assert.strictEqual(mRes.status, 0, mRes.err);
  assert.strictEqual(mRes.out, 'vaughan 0.2.6');

  const resolved = resolveBinary({
    platform: 'linux',
    arch: 'x64',
    resolve: (spec) => {
      if (spec === 'vaughan-linux-x64/bin/vaughan') return pkgBin;
      throw new Error(`unexpected resolve ${spec}`);
    },
  });
  assert.strictEqual(resolved.binary, pkgBin);
  assert.strictEqual(resolved.pkg, 'vaughan-linux-x64');
});

check('vaughan bin passes args straight through and propagates exit code', () => {
  const { out, status } = runScript(
    vaughanBin,
    ['send', '0x1234', '--value', '0.1'],
    { VAUGHAN_BIN: fake }
  );
  assert.strictEqual(status, 0);
  assert.deepStrictEqual(out.split('\n'), ['send', '0x1234', '--value', '0.1']);

  const failing = path.join(tmpRoot, 'vaughan-fail');
  fs.writeFileSync(failing, '#!/bin/sh\nexit 9\n', { mode: 0o755 });
  const failRes = runScript(vaughanBin, ['balance'], { VAUGHAN_BIN: failing });
  assert.strictEqual(failRes.status, 9);
});

check('vaughan-mcp bin execs `vaughan mcp` and handles VAUGHAN_PROFILE + exit code', () => {
  const r1 = runScript(vaughanMcpBin, ['--profile', 'sentient'], { VAUGHAN_BIN: fake });
  assert.strictEqual(r1.status, 0);
  assert.deepStrictEqual(r1.out.split('\n'), ['mcp', '--profile', 'sentient']);

  const r2 = runScript(vaughanMcpBin, [], {
    VAUGHAN_BIN: fake,
    VAUGHAN_PROFILE: 'sentient',
  });
  assert.strictEqual(r2.status, 0);
  assert.deepStrictEqual(r2.out.split('\n'), ['mcp', '--profile', 'sentient']);

  const r3 = runScript(vaughanMcpBin, ['--profile', 'default'], {
    VAUGHAN_BIN: fake,
    VAUGHAN_PROFILE: 'sentient',
  });
  assert.strictEqual(r3.status, 0);
  assert.deepStrictEqual(r3.out.split('\n'), ['mcp', '--profile', 'default']);

  const failing = path.join(tmpRoot, 'vaughan-mcp-fail');
  fs.writeFileSync(failing, '#!/bin/sh\nexit 13\n', { mode: 0o755 });
  const r4 = runScript(vaughanMcpBin, [], { VAUGHAN_BIN: failing });
  assert.strictEqual(r4.status, 13);
});

check('--version works on both vaughan and vaughan-mcp bins', () => {
  const vOut = runScript(vaughanBin, ['--version'], { VAUGHAN_BIN: fake });
  assert.strictEqual(vOut.status, 0);
  assert.strictEqual(vOut.out, 'vaughan 0.2.6');

  const mOut = runScript(vaughanMcpBin, ['--version'], { VAUGHAN_BIN: fake });
  assert.strictEqual(mOut.status, 0);
  assert.strictEqual(mOut.out, 'vaughan 0.2.6');
});

check('launcher package.json pins optionalDependencies to exact version and has no postinstall', () => {
  const launcherPkg = JSON.parse(fs.readFileSync(path.join(launcherRoot, 'package.json'), 'utf8'));
  const mcpPkg = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'package.json'), 'utf8'));

  assert.strictEqual(launcherPkg.name, 'vaughan');
  assert.strictEqual(launcherPkg.version, mcpPkg.version);
  assert.strictEqual(launcherPkg.bin.vaughan, 'bin/vaughan.js');
  assert.strictEqual(launcherPkg.bin['vaughan-mcp'], 'bin/vaughan-mcp.js');
  assert.ok(
    !launcherPkg.scripts ||
      (!launcherPkg.scripts.postinstall && !launcherPkg.scripts.install && !launcherPkg.scripts.preinstall),
    'launcher must not define install/postinstall scripts'
  );

  const expectedDeps = ['vaughan-darwin-arm64', 'vaughan-linux-arm64', 'vaughan-linux-x64'];
  assert.deepStrictEqual(Object.keys(launcherPkg.optionalDependencies).sort(), expectedDeps);
  for (const dep of expectedDeps) {
    assert.strictEqual(
      launcherPkg.optionalDependencies[dep],
      launcherPkg.version,
      `optionalDependency ${dep} must match exact version ${launcherPkg.version}`
    );
  }
});

fs.rmSync(tmpRoot, { recursive: true, force: true });
console.log(failures === 0 ? '\nall launcher tests passed' : `\n${failures} test(s) failed`);
process.exit(failures === 0 ? 0 : 1);
