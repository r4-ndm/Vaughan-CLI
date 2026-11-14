'use strict';
// Platform-package resolution for the `vaughan` npm launcher.
// No postinstall and no runtime network download: npm installs the matching
// platform package via optionalDependencies and pins its integrity metadata.

const fs = require('fs');
const path = require('path');

const REPO = 'r4-ndm/Vaughan-CLI';
const RELEASES_URL = `https://github.com/${REPO}/releases`;

const PLATFORM_PACKAGES = Object.freeze({
  'linux-x64': Object.freeze({
    pkg: 'vaughan-linux-x64',
    tarball: 'vaughan-linux-x86_64.tar.gz',
    os: 'linux',
    cpu: 'x64',
  }),
  'linux-arm64': Object.freeze({
    pkg: 'vaughan-linux-arm64',
    tarball: 'vaughan-linux-aarch64.tar.gz',
    os: 'linux',
    cpu: 'arm64',
  }),
  'darwin-arm64': Object.freeze({
    pkg: 'vaughan-darwin-arm64',
    tarball: 'vaughan-macos-aarch64.tar.gz',
    os: 'darwin',
    cpu: 'arm64',
  }),
});

function resolvePlatformPackage(platform = process.platform, arch = process.arch) {
  const key = `${platform}-${arch}`;
  const entry = PLATFORM_PACKAGES[key];
  if (entry) {
    return { key, ...entry };
  }
  const supported = Object.keys(PLATFORM_PACKAGES).join(', ');
  const err = new Error(
    `UNSUPPORTED_PLATFORM: ${key} is not supported via npm (supported: ${supported}). ` +
      `Download a checksum-verified release tarball or build from source at ${RELEASES_URL} ` +
      `(or run: curl -fsSL https://raw.githubusercontent.com/${REPO}/main/scripts/install.sh | sh).`
  );
  err.code = 'UNSUPPORTED_PLATFORM';
  return { key, error: err };
}

function resolveBinary(options = {}) {
  const env = options.env || process.env;
  const platform = options.platform || env.VAUGHAN_TEST_PLATFORM || process.platform;
  const arch = options.arch || env.VAUGHAN_TEST_ARCH || process.arch;
  const customResolve = options.resolve || require.resolve;

  if (env.VAUGHAN_BIN) {
    const explicit = env.VAUGHAN_BIN;
    if (!fs.existsSync(explicit)) {
      const err = new Error(`VAUGHAN_BIN points at ${explicit}, which does not exist`);
      err.code = 'VAUGHAN_BIN_MISSING';
      return { error: err };
    }
    return { binary: explicit, key: `${platform}-${arch}`, pkg: null };
  }

  const target = resolvePlatformPackage(platform, arch);
  if (target.error) {
    return { error: target.error, key: target.key };
  }

  let binaryPath = null;
  try {
    binaryPath = customResolve(`${target.pkg}/bin/vaughan`);
  } catch (_) {
    try {
      const pkgJsonPath = customResolve(`${target.pkg}/package.json`);
      binaryPath = path.join(path.dirname(pkgJsonPath), 'bin', 'vaughan');
    } catch (resolveErr) {
      const err = new Error(
        `UNSUPPORTED_PLATFORM: platform package ${target.pkg} for ${target.key} is not installed ` +
          `(was npm run with --omit=optional / --no-optional?). ` +
          `Install from ${RELEASES_URL} (${target.tarball}) or re-install without --no-optional.`
      );
      err.code = 'PLATFORM_PACKAGE_MISSING';
      err.cause = resolveErr;
      return { error: err, key: target.key, pkg: target.pkg };
    }
  }

  if (!binaryPath || !fs.existsSync(binaryPath)) {
    const err = new Error(
      `platform package ${target.pkg} resolved to ${binaryPath}, which does not exist. ` +
        `Install from ${RELEASES_URL} (${target.tarball}).`
    );
    err.code = 'PLATFORM_BINARY_MISSING';
    return { error: err, key: target.key, pkg: target.pkg };
  }

  return { binary: binaryPath, key: target.key, pkg: target.pkg };
}

module.exports = {
  REPO,
  RELEASES_URL,
  PLATFORM_PACKAGES,
  resolvePlatformPackage,
  resolveBinary,
};
