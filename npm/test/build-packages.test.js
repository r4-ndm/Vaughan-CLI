'use strict';
// Offline unit tests for scripts/npm-build-packages.mjs (positive build + fail-closed gates):
//   node test/build-packages.test.js
const assert = require('assert');
const crypto = require('crypto');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const repoRoot = path.resolve(__dirname, '..', '..');
const builderScript = path.join(repoRoot, 'scripts', 'npm-build-packages.mjs');

const TARBALLS = [
  'vaughan-linux-x86_64.tar.gz',
  'vaughan-linux-aarch64.tar.gz',
  'vaughan-macos-aarch64.tar.gz',
];

function sha256(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

function makeFixture(baseDir, opts = {}) {
  const artifactsDir = path.join(baseDir, 'artifacts');
  const srcDir = path.join(baseDir, 'bin-src');
  fs.mkdirSync(artifactsDir, { recursive: true });
  fs.mkdirSync(srcDir, { recursive: true });

  const sumLines = [];
  for (const asset of TARBALLS) {
    const binName = opts.wrongBinaryNameFor === asset ? 'not-vaughan' : 'vaughan';
    const stage = path.join(srcDir, asset);
    fs.mkdirSync(stage, { recursive: true });
    const fakeBin = path.join(stage, binName);
    fs.writeFileSync(
      fakeBin,
      `#!/bin/sh\necho "vaughan 0.2.6 (${asset})"\n`,
      { mode: 0o755 }
    );
    const tbPath = path.join(artifactsDir, asset);
    const tarRes = spawnSync('tar', ['-czf', tbPath, '-C', stage, binName], {
      encoding: 'utf8',
    });
    assert.strictEqual(tarRes.status, 0, tarRes.stderr);
    sumLines.push(`${sha256(tbPath)}  ${asset}`);
  }

  fs.writeFileSync(path.join(artifactsDir, 'SHA256SUMS'), `${sumLines.join('\n')}\n`, 'utf8');
  return artifactsDir;
}

function runBuilder(tag, artifactsDir, outDir) {
  const res = spawnSync(process.execPath, [builderScript, tag, artifactsDir, outDir], {
    encoding: 'utf8',
  });
  return {
    status: res.status,
    out: (res.stdout || '').trim(),
    err: (res.stderr || '').trim(),
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

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'vaughan-npm-builder-test-'));

check('builds verified platform packages and launcher with pinned versions', () => {
  const caseDir = path.join(tmpRoot, 'positive');
  const artifactsDir = makeFixture(caseDir);
  const outDir = path.join(caseDir, 'dist');

  const res = runBuilder('v0.2.6', artifactsDir, outDir);
  assert.strictEqual(res.status, 0, res.err);

  const expectedMeta = {
    'vaughan-linux-x64': { os: ['linux'], cpu: ['x64'] },
    'vaughan-linux-arm64': { os: ['linux'], cpu: ['arm64'] },
    'vaughan-darwin-arm64': { os: ['darwin'], cpu: ['arm64'] },
  };

  for (const [pkgName, meta] of Object.entries(expectedMeta)) {
    const pkgDir = path.join(outDir, pkgName);
    const pkgJson = JSON.parse(fs.readFileSync(path.join(pkgDir, 'package.json'), 'utf8'));
    assert.strictEqual(pkgJson.name, pkgName);
    assert.strictEqual(pkgJson.version, '0.2.6');
    assert.deepStrictEqual(pkgJson.os, meta.os);
    assert.deepStrictEqual(pkgJson.cpu, meta.cpu);

    const binPath = path.join(pkgDir, 'bin', 'vaughan');
    const mode = fs.statSync(binPath).mode & 0o777;
    assert.strictEqual(mode, 0o755, `${pkgName}/bin/vaughan expected 0755, got ${mode.toString(8)}`);

    const execOut = spawnSync(binPath, ['--version'], { encoding: 'utf8' });
    assert.strictEqual(execOut.status, 0);
    assert.ok(execOut.stdout.includes('vaughan 0.2.6'), execOut.stdout);
  }

  const launcherDir = path.join(outDir, 'vaughan');
  const launcherPkg = JSON.parse(fs.readFileSync(path.join(launcherDir, 'package.json'), 'utf8'));
  assert.strictEqual(launcherPkg.name, 'vaughan');
  assert.strictEqual(launcherPkg.version, '0.2.6');
  assert.deepStrictEqual(launcherPkg.optionalDependencies, {
    'vaughan-darwin-arm64': '0.2.6',
    'vaughan-linux-arm64': '0.2.6',
    'vaughan-linux-x64': '0.2.6',
  });
  assert.ok(fs.existsSync(path.join(launcherDir, 'bin', 'vaughan.js')));
  assert.ok(fs.existsSync(path.join(launcherDir, 'bin', 'vaughan-mcp.js')));
  assert.ok(fs.existsSync(path.join(launcherDir, 'lib', 'resolve.js')));
});

check('fails closed when a platform tarball is missing', () => {
  const caseDir = path.join(tmpRoot, 'missing-tarball');
  const artifactsDir = makeFixture(caseDir);
  fs.rmSync(path.join(artifactsDir, 'vaughan-macos-aarch64.tar.gz'));
  const outDir = path.join(caseDir, 'dist');

  const res = runBuilder('v0.2.6', artifactsDir, outDir);
  assert.notStrictEqual(res.status, 0, 'expected non-zero exit');
  assert.ok(/missing release tarball.*vaughan-macos-aarch64\.tar\.gz/.test(res.err), res.err);
  assert.ok(!fs.existsSync(outDir), 'must not emit dist on missing tarball');
});

check('fails closed when SHA256SUMS is missing an entry', () => {
  const caseDir = path.join(tmpRoot, 'missing-sum-line');
  const artifactsDir = makeFixture(caseDir);
  const lines = fs
    .readFileSync(path.join(artifactsDir, 'SHA256SUMS'), 'utf8')
    .split('\n')
    .filter((l) => !l.includes('vaughan-linux-aarch64.tar.gz'))
    .join('\n');
  fs.writeFileSync(path.join(artifactsDir, 'SHA256SUMS'), lines, 'utf8');
  const outDir = path.join(caseDir, 'dist');

  const res = runBuilder('v0.2.6', artifactsDir, outDir);
  assert.notStrictEqual(res.status, 0, 'expected non-zero exit');
  assert.ok(/missing checksum line.*vaughan-linux-aarch64\.tar\.gz/.test(res.err), res.err);
  assert.ok(!fs.existsSync(outDir), 'must not emit dist on missing checksum line');
});

check('fails closed when a tarball checksum mismatches (corrupted tarball)', () => {
  const caseDir = path.join(tmpRoot, 'corrupt-tarball');
  const artifactsDir = makeFixture(caseDir);
  fs.appendFileSync(path.join(artifactsDir, 'vaughan-linux-x86_64.tar.gz'), 'CORRUPT');
  const outDir = path.join(caseDir, 'dist');

  const res = runBuilder('v0.2.6', artifactsDir, outDir);
  assert.notStrictEqual(res.status, 0, 'expected non-zero exit');
  assert.ok(/checksum mismatch.*vaughan-linux-x86_64\.tar\.gz/.test(res.err), res.err);
  assert.ok(!fs.existsSync(outDir), 'must not emit dist on checksum mismatch');
});

check('fails closed when SHA256SUMS file is missing', () => {
  const caseDir = path.join(tmpRoot, 'missing-shasums');
  const artifactsDir = makeFixture(caseDir);
  fs.rmSync(path.join(artifactsDir, 'SHA256SUMS'));
  const outDir = path.join(caseDir, 'dist');

  const res = runBuilder('v0.2.6', artifactsDir, outDir);
  assert.notStrictEqual(res.status, 0, 'expected non-zero exit');
  assert.ok(/missing SHA256SUMS/.test(res.err), res.err);
  assert.ok(!fs.existsSync(outDir), 'must not emit dist without SHA256SUMS');
});

check('fails closed when an archive lacks the vaughan binary', () => {
  const caseDir = path.join(tmpRoot, 'missing-binary-in-tar');
  const artifactsDir = makeFixture(caseDir, {
    wrongBinaryNameFor: 'vaughan-linux-x86_64.tar.gz',
  });
  const outDir = path.join(caseDir, 'dist');

  const res = runBuilder('v0.2.6', artifactsDir, outDir);
  assert.notStrictEqual(res.status, 0, 'expected non-zero exit');
  assert.ok(/did not contain.*vaughan/.test(res.err), res.err);
  assert.ok(!fs.existsSync(outDir), 'must not emit dist when binary is absent');
});

fs.rmSync(tmpRoot, { recursive: true, force: true });
console.log(failures === 0 ? '\nall build-packages tests passed' : `\n${failures} test(s) failed`);
process.exit(failures === 0 ? 0 : 1);
