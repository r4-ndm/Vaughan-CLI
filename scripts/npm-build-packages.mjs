#!/usr/bin/env node
/**
 * Assemble platform-specific npm packages (`vaughan-linux-x64`, `vaughan-linux-arm64`,
 * `vaughan-darwin-arm64`) and the `vaughan` launcher package from verified GitHub
 * release tarballs + `SHA256SUMS`.
 *
 * Fail-closed: aborts with non-zero exit (and leaves no partial `outDir`) if
 * `SHA256SUMS` is missing, any expected tarball is missing or corrupted, or an
 * archive does not contain a non-empty `vaughan` binary.
 *
 * Usage:
 *   node scripts/npm-build-packages.mjs <tag> <artifacts-dir> [out-dir]
 *   node scripts/npm-build-packages.mjs --tag v0.2.6 --artifacts ./release-assets --out ./npm/dist
 */

import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const REPO_ROOT = path.resolve(__dirname, '..');
const DEFAULT_LAUNCHER_DIR = path.join(REPO_ROOT, 'npm', 'launcher');
const DEFAULT_OUT_DIR = path.join(REPO_ROOT, 'npm', 'dist');

export const TARGETS = Object.freeze([
  Object.freeze({
    pkg: 'vaughan-linux-x64',
    tarball: 'vaughan-linux-x86_64.tar.gz',
    os: 'linux',
    cpu: 'x64',
    label: 'Linux x86_64',
  }),
  Object.freeze({
    pkg: 'vaughan-linux-arm64',
    tarball: 'vaughan-linux-aarch64.tar.gz',
    os: 'linux',
    cpu: 'arm64',
    label: 'Linux ARM64',
  }),
  Object.freeze({
    pkg: 'vaughan-darwin-arm64',
    tarball: 'vaughan-macos-aarch64.tar.gz',
    os: 'darwin',
    cpu: 'arm64',
    label: 'macOS Apple Silicon (arm64)',
  }),
]);

export function parseVersionFromTag(tag) {
  if (typeof tag !== 'string' || !/^v?\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(tag.trim())) {
    throw new Error(`invalid release tag "${tag}" (expected e.g. v0.2.6 or 0.2.6)`);
  }
  return tag.trim().replace(/^v/, '');
}

export function parseShasumsFile(text) {
  if (typeof text !== 'string') {
    throw new Error('SHA256SUMS content must be a string');
  }
  const map = new Map();
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) continue;
    const match = line.match(/^([0-9a-fA-F]{64})\s+\*?(.+)$/);
    if (!match) continue;
    const digest = match[1].toLowerCase();
    const assetName = match[2].trim().replace(/^\.\//, '');
    if (map.has(assetName) && map.get(assetName) !== digest) {
      throw new Error(`conflicting checksum entries in SHA256SUMS for ${assetName}`);
    }
    map.set(assetName, digest);
  }
  return map;
}

function sha256File(filePath) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(filePath));
  return hash.digest('hex');
}

function findExtractedBinary(extractDir, tarballName) {
  const direct = path.join(extractDir, 'vaughan');
  if (fs.existsSync(direct)) {
    const st = fs.lstatSync(direct);
    if (!st.isSymbolicLink() && st.isFile() && st.size > 0) {
      return direct;
    }
    throw new Error(`tarball ${tarballName} contains an invalid or empty vaughan entry`);
  }

  const nestedCandidates = fs
    .readdirSync(extractDir, { withFileTypes: true })
    .filter((d) => d.isDirectory())
    .map((d) => path.join(extractDir, d.name, 'vaughan'))
    .filter((p) => {
      if (!fs.existsSync(p)) return false;
      const st = fs.lstatSync(p);
      return !st.isSymbolicLink() && st.isFile() && st.size > 0;
    });

  if (nestedCandidates.length === 1) {
    return nestedCandidates[0];
  }

  throw new Error(`tarball ${tarballName} did not contain a valid non-empty vaughan binary`);
}

function extractTarball(tarballPath, extractDir, tarballName) {
  fs.mkdirSync(extractDir, { recursive: true });
  const res = spawnSync('tar', ['-xzf', tarballPath, '-C', extractDir], { encoding: 'utf8' });
  if (res.status !== 0) {
    const stderr = (res.stderr || '').trim();
    throw new Error(`failed to extract ${tarballName} (exit ${res.status}): ${stderr}`);
  }
  return findExtractedBinary(extractDir, tarballName);
}

function buildPlatformPackageJson(target, version) {
  return {
    name: target.pkg,
    version,
    description: `Vaughan CLI native binary for ${target.label}. Installed automatically by the \`vaughan\` npm package.`,
    license: 'MIT',
    os: [target.os],
    cpu: [target.cpu],
    files: ['bin/vaughan', 'README.md'],
    preferUnplugged: true,
    repository: {
      type: 'git',
      url: 'git+https://github.com/r4-ndm/Vaughan-CLI.git',
    },
  };
}

function buildPlatformReadme(target) {
  return [
    `# ${target.pkg}`,
    '',
    `Prebuilt native \`vaughan\` binary for **${target.label}** (${target.os}/${target.cpu}).`,
    '',
    'This package is an `optionalDependency` of [`vaughan`](https://www.npmjs.com/package/vaughan) and is not meant to be invoked directly.',
    '',
    'For funded wallets, install the checksum-verified GitHub Release tarball instead:',
    'https://github.com/r4-ndm/Vaughan-CLI/releases',
    '',
  ].join('\n');
}

export function buildNpmPackages({
  tag,
  artifactsDir,
  outDir = DEFAULT_OUT_DIR,
  launcherDir = DEFAULT_LAUNCHER_DIR,
}) {
  const version = parseVersionFromTag(tag);
  const absArtifacts = path.resolve(artifactsDir);
  const absOut = path.resolve(outDir);
  const absLauncher = path.resolve(launcherDir);

  if (!fs.existsSync(absArtifacts) || !fs.statSync(absArtifacts).isDirectory()) {
    throw new Error(`artifacts directory does not exist: ${absArtifacts}`);
  }
  if (!fs.existsSync(path.join(absLauncher, 'package.json'))) {
    throw new Error(`launcher template missing package.json at ${absLauncher}`);
  }

  const shasumsPath = path.join(absArtifacts, 'SHA256SUMS');
  if (!fs.existsSync(shasumsPath) || !fs.statSync(shasumsPath).isFile()) {
    throw new Error(`missing SHA256SUMS in artifacts directory: ${shasumsPath}`);
  }
  const shasums = parseShasumsFile(fs.readFileSync(shasumsPath, 'utf8'));

  const outParent = path.dirname(absOut);
  fs.mkdirSync(outParent, { recursive: true });
  const workRoot = fs.mkdtempSync(path.join(outParent, '.npm-pkg-build-'));
  const stageDir = path.join(workRoot, 'dist');
  fs.mkdirSync(stageDir, { recursive: true });

  try {
    // 1. Verify and assemble all three platform packages (fail closed on any gap).
    for (const target of TARGETS) {
      const tarballPath = path.join(absArtifacts, target.tarball);
      if (!fs.existsSync(tarballPath) || !fs.statSync(tarballPath).isFile()) {
        throw new Error(`missing release tarball: ${target.tarball} (expected at ${tarballPath})`);
      }

      const expectedDigest = shasums.get(target.tarball);
      if (!expectedDigest) {
        throw new Error(`missing checksum line in SHA256SUMS for ${target.tarball}`);
      }

      const actualDigest = sha256File(tarballPath);
      if (actualDigest !== expectedDigest) {
        throw new Error(
          `checksum mismatch for ${target.tarball}: expected ${expectedDigest}, got ${actualDigest}`
        );
      }

      const extractDir = path.join(workRoot, 'extract', target.pkg);
      const extractedBinary = extractTarball(tarballPath, extractDir, target.tarball);

      const pkgDir = path.join(stageDir, target.pkg);
      const binDir = path.join(pkgDir, 'bin');
      fs.mkdirSync(binDir, { recursive: true });

      const destBinary = path.join(binDir, 'vaughan');
      fs.copyFileSync(extractedBinary, destBinary);
      fs.chmodSync(destBinary, 0o755);

      fs.writeFileSync(
        path.join(pkgDir, 'package.json'),
        `${JSON.stringify(buildPlatformPackageJson(target, version), null, 2)}\n`,
        'utf8'
      );
      fs.writeFileSync(path.join(pkgDir, 'README.md'), buildPlatformReadme(target), 'utf8');
    }

    // 2. Assemble the `vaughan` launcher package pinned to the exact platform versions.
    const launcherOutDir = path.join(stageDir, 'vaughan');
    fs.mkdirSync(path.join(launcherOutDir, 'bin'), { recursive: true });
    fs.mkdirSync(path.join(launcherOutDir, 'lib'), { recursive: true });

    const launcherPkg = JSON.parse(
      fs.readFileSync(path.join(absLauncher, 'package.json'), 'utf8')
    );
    launcherPkg.version = version;
    const pinnedOptionalDeps = {};
    for (const target of [...TARGETS].sort((a, b) => a.pkg.localeCompare(b.pkg))) {
      pinnedOptionalDeps[target.pkg] = version;
    }
    launcherPkg.optionalDependencies = pinnedOptionalDeps;
    if (launcherPkg.scripts && (launcherPkg.scripts.postinstall || launcherPkg.scripts.install)) {
      throw new Error('launcher package.json must not define install/postinstall scripts');
    }

    fs.writeFileSync(
      path.join(launcherOutDir, 'package.json'),
      `${JSON.stringify(launcherPkg, null, 2)}\n`,
      'utf8'
    );

    const filesToCopy = [
      { rel: path.join('bin', 'vaughan.js'), mode: 0o755 },
      { rel: path.join('bin', 'vaughan-mcp.js'), mode: 0o755 },
      { rel: path.join('lib', 'resolve.js'), mode: 0o644 },
      { rel: 'README.md', mode: 0o644 },
    ];
    for (const item of filesToCopy) {
      const src = path.join(absLauncher, item.rel);
      const dst = path.join(launcherOutDir, item.rel);
      if (!fs.existsSync(src)) {
        throw new Error(`launcher file missing: ${src}`);
      }
      fs.copyFileSync(src, dst);
      fs.chmodSync(dst, item.mode);
    }

    // 3. Promote staged packages atomically into outDir.
    fs.rmSync(absOut, { recursive: true, force: true });
    fs.renameSync(stageDir, absOut);

    return {
      version,
      outDir: absOut,
      packages: [...TARGETS.map((t) => t.pkg), 'vaughan'],
    };
  } finally {
    fs.rmSync(workRoot, { recursive: true, force: true });
  }
}

function parseCliArgs(argv) {
  let tag = null;
  let artifactsDir = null;
  let outDir = DEFAULT_OUT_DIR;
  const positional = [];

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '-h' || arg === '--help') {
      return { help: true };
    }
    if (arg === '--tag') {
      tag = argv[++i];
    } else if (arg === '--artifacts') {
      artifactsDir = argv[++i];
    } else if (arg === '--out') {
      outDir = argv[++i];
    } else if (arg.startsWith('-')) {
      throw new Error(`unknown option: ${arg}`);
    } else {
      positional.push(arg);
    }
  }

  if (!tag && positional.length >= 1) tag = positional[0];
  if (!artifactsDir && positional.length >= 2) artifactsDir = positional[1];
  if (outDir === DEFAULT_OUT_DIR && positional.length >= 3) outDir = positional[2];

  if (!tag || !artifactsDir) {
    throw new Error(
      'usage: node scripts/npm-build-packages.mjs <tag> <artifacts-dir> [out-dir]'
    );
  }
  return { tag, artifactsDir, outDir };
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(__filename)) {
  try {
    const parsed = parseCliArgs(process.argv.slice(2));
    if (parsed.help) {
      process.stdout.write(
        'Usage: node scripts/npm-build-packages.mjs <tag> <artifacts-dir> [out-dir]\n'
      );
      process.exit(0);
    }
    const result = buildNpmPackages(parsed);
    process.stdout.write(
      `Built npm packages for v${result.version} in ${result.outDir}: ${result.packages.join(', ')}\n`
    );
  } catch (err) {
    process.stderr.write(`npm-build-packages ERROR: ${err.message}\n`);
    process.exit(1);
  }
}
