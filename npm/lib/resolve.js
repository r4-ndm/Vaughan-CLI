'use strict';
// Platform resolution for the Vaughan binary. Kept dependency-free and pure so the
// mapping can be unit-tested without a network or a release.

// The release workflow (.github/workflows/release.yml) publishes exactly these
// three tarballs plus SHA256SUMS. Intel macOS is intentionally absent: those
// runners are deprecated, and an x86_64 mac can run the arm64 binary under
// Rosetta or build from source.
const TARGETS = {
  'linux-x64': 'vaughan-linux-x86_64.tar.gz',
  'linux-arm64': 'vaughan-linux-aarch64.tar.gz',
  'darwin-arm64': 'vaughan-macos-aarch64.tar.gz',
};

const REPO = 'r4-ndm/Vaughan-CLI';

function resolveTarget(platform = process.platform, arch = process.arch) {
  const key = `${platform}-${arch}`;
  const asset = TARGETS[key];
  if (asset) return { asset, key };
  // Fail with the reason and the two ways out, never with a stack trace: an
  // agent reading this needs an actionable line, not a node crash.
  const known = Object.keys(TARGETS).join(', ');
  const err = new Error(
    `unsupported platform ${key} — Vaughan publishes ${known}. ` +
      `Build from source (cargo install --path vaughan-cli) and set VAUGHAN_BIN=/path/to/vaughan.`
  );
  err.code = 'UNSUPPORTED_PLATFORM';
  return { error: err };
}

// SHA256SUMS format from the release job: "<hex>  <filename>" per line.
// Returns the hex digest for one asset, or null when the file does not list it.
function parseShasums(text, asset) {
  if (typeof text !== 'string') return null;
  for (const rawLine of text.split('\n')) {
    const line = rawLine.trim();
    if (!line) continue;
    const match = line.match(/^([0-9a-fA-F]{64})\s+\*?(.+)$/);
    if (!match) continue;
    if (match[2].trim() === asset) return match[1].toLowerCase();
  }
  return null;
}

function downloadUrl(version, asset) {
  if (!/^v?\d+\.\d+\.\d+/.test(String(version))) {
    throw new Error(`refusing to build a download URL from version "${version}"`);
  }
  const tag = String(version).startsWith('v') ? version : `v${version}`;
  return `https://github.com/${REPO}/releases/download/${tag}/${asset}`;
}

module.exports = { TARGETS, REPO, resolveTarget, parseShasums, downloadUrl };
