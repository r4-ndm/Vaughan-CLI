#!/usr/bin/env node
'use strict';
// vaughan-mcp — fetch the pinned Vaughan release binary once, verify its checksum,
// then exec `vaughan mcp <args...>`.
//
// Why a wrapper instead of shipping the binary in the package: the binary is ~15 MB
// per platform, the release job already produces it with SHA256SUMS, and an agent
// runtime that speaks `npx` should be able to reach Vaughan without a Rust toolchain.
//
// Environment:
//   VAUGHAN_BIN      absolute path to a vaughan binary to use instead of downloading
//   VAUGHAN_VERSION  release tag to fetch (default: this package's version)
//   VAUGHAN_CACHE    cache root (default: ~/.cache/vaughan-mcp)
//   VAUGHAN_NO_VERIFY=1  skip the SHA256 check — only for a local mirror of a build
//                         you made yourself. Never set this when fetching a release.

const fs = require('fs');
const os = require('os');
const path = require('path');
const https = require('https');
const crypto = require('crypto');
const { spawn, spawnSync } = require('child_process');

const { resolveTarget, parseShasums, downloadUrl } = require('../lib/resolve');

const VERSION = process.env.VAUGHAN_VERSION || require('../package.json').version;
const CACHE_ROOT = process.env.VAUGHAN_CACHE || path.join(os.homedir(), '.cache', 'vaughan-mcp');

function die(message, code = 1) {
  process.stderr.write(`vaughan-mcp: ${message}\n`);
  process.exit(code);
}

function get(url, redirectsLeft = 5) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { 'user-agent': 'vaughan-mcp-npm' } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          if (redirectsLeft <= 0) return reject(new Error('too many redirects'));
          res.resume();
          return resolve(get(res.headers.location, redirectsLeft - 1));
        }
        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`GET ${url} -> HTTP ${res.statusCode}`));
        }
        resolve(res);
      })
      .on('error', reject);
  });
}

async function downloadTo(url, destFile) {
  const res = await get(url);
  await new Promise((resolve, reject) => {
    const out = fs.createWriteStream(destFile, { mode: 0o600 });
    res.pipe(out);
    out.on('finish', resolve);
    out.on('error', reject);
    res.on('error', reject);
  });
}

async function downloadText(url) {
  const res = await get(url);
  return new Promise((resolve, reject) => {
    let data = '';
    res.setEncoding('utf8');
    res.on('data', (chunk) => (data += chunk));
    res.on('end', () => resolve(data));
    res.on('error', reject);
  });
}

function sha256File(file) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(file));
  return hash.digest('hex');
}

function extract(tarball, intoDir) {
  fs.mkdirSync(intoDir, { recursive: true });
  // `tar` is present on every supported platform; adding a JS tar parser would be
  // a dependency for one call.
  const res = spawnSync('tar', ['-xzf', tarball, '-C', intoDir], { encoding: 'utf8' });
  if (res.status !== 0) {
    throw new Error(`tar failed (${res.status}): ${(res.stderr || '').trim()}`);
  }
  // The archive may or may not nest the binary in a directory.
  const candidate = path.join(intoDir, 'vaughan');
  if (fs.existsSync(candidate)) return candidate;
  const found = fs
    .readdirSync(intoDir, { withFileTypes: true })
    .filter((d) => d.isFile() && d.name === 'vaughan')
    .map((d) => path.join(intoDir, d.name));
  if (found.length === 1) return found[0];
  const nested = fs
    .readdirSync(intoDir, { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => path.join(intoDir, d.name, 'vaughan'))
    .filter((p) => fs.existsSync(p));
  if (nested.length === 1) return nested[0];
  throw new Error('archive did not contain a `vaughan` binary');
}

async function ensureBinary() {
  if (process.env.VAUGHAN_BIN) {
    const explicit = process.env.VAUGHAN_BIN;
    if (!fs.existsSync(explicit)) die(`VAUGHAN_BIN points at ${explicit}, which does not exist`);
    return explicit;
  }

  const { asset, key, error } = resolveTarget();
  if (error) die(error.message);

  const versionDir = path.join(CACHE_ROOT, VERSION.replace(/^v/, ''));
  const binary = path.join(versionDir, 'vaughan');
  if (fs.existsSync(binary)) return binary;

  fs.mkdirSync(versionDir, { recursive: true });
  const tarball = path.join(versionDir, `${asset}.part`);

  try {
    process.stderr.write(`vaughan-mcp: fetching ${asset} (${VERSION})…\n`);
    await downloadTo(downloadUrl(VERSION, asset), tarball);

    if (process.env.VAUGHAN_NO_VERIFY !== '1') {
      const sums = await downloadText(downloadUrl(VERSION, 'SHA256SUMS'));
      const expected = parseShasums(sums, asset);
      if (!expected) {
        throw new Error(`SHA256SUMS does not list ${asset}; refusing to install an unverified binary`);
      }
      const actual = sha256File(tarball);
      if (actual !== expected) {
        throw new Error(`checksum mismatch for ${asset}: expected ${expected}, got ${actual}`);
      }
    }

    const staged = fs.mkdtempSync(path.join(versionDir, '.stage-'));
    const extracted = extract(tarball, staged);
    fs.chmodSync(extracted, 0o755);
    fs.renameSync(extracted, binary);
    fs.rmSync(staged, { recursive: true, force: true });
    // The tarball is only an intermediate: keeping it wastes ~2.7 MB per version and
    // a stale .part next to a good binary reads as a failed install.
    fs.rmSync(tarball, { force: true });
    return binary;
  } catch (e) {
    // Never leave a half-downloaded binary that a later run would trust.
    fs.rmSync(tarball, { force: true });
    fs.rmSync(binary, { force: true });
    die(
      `${e.message}\n` +
        `No binary installed. Options:\n` +
        `  1. retry (transient network failures are common)\n` +
        `  2. curl -fsSL https://raw.githubusercontent.com/r4-ndm/Vaughan-CLI/main/scripts/install.sh | sh\n` +
        `  3. build from source and set VAUGHAN_BIN=/path/to/vaughan\n` +
        `Target: ${key || 'unknown'} — cache: ${versionDir}`
    );
  }
}

async function main() {
  const args = process.argv.slice(2);

  if (args.includes('--wrapper-version')) {
    const binary = fs.existsSync(path.join(CACHE_ROOT, VERSION.replace(/^v/, ''), 'vaughan'))
      ? path.join(CACHE_ROOT, VERSION.replace(/^v/, ''), 'vaughan')
      : '(not downloaded yet)';
    process.stdout.write(
      `vaughan-mcp ${VERSION}\nplatform ${process.platform}-${process.arch}\ncached binary: ${binary}\n`
    );
    return 0;
  }

  const binary = await ensureBinary();

  // VAUGHAN_PROFILE keeps the client config short and lets a runtime switch profiles
  // without editing JSON. An explicit --profile on the command line still wins.
  const forwarded = args.slice();
  if (process.env.VAUGHAN_PROFILE && !forwarded.includes('--profile')) {
    forwarded.push('--profile', process.env.VAUGHAN_PROFILE);
  }

  // stdio MCP: the client speaks JSON-RPC over this process's stdin/stdout, so the
  // wrapper must not print anything to stdout and must hand the child the real fds.
  const child = spawn(binary, ['mcp', ...forwarded], { stdio: 'inherit' });
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    process.on(signal, () => child.kill(signal));
  }
  return await new Promise((resolve) => {
    child.on('exit', (code, signal) => resolve(signal ? 1 : code ?? 0));
    child.on('error', (e) => {
      process.stderr.write(`vaughan-mcp: failed to start ${binary}: ${e.message}\n`);
      resolve(1);
    });
  });
}

if (require.main === module) {
  main().then((code) => process.exit(code));
}

module.exports = { ensureBinary, main };
