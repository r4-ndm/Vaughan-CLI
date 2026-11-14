#!/usr/bin/env node
'use strict';
// vaughan — human CLI/TUI launcher. Resolves the platform package installed via
// optionalDependencies and spawns the native `vaughan` binary with args passed
// straight through, stdio inherited, and exit code propagated.

const { spawn } = require('child_process');
const { resolveBinary } = require('../lib/resolve');

function main() {
  const { binary, error } = resolveBinary();
  if (error) {
    process.stderr.write(`vaughan: ${error.message}\n`);
    process.exit(1);
  }

  const args = process.argv.slice(2);
  const child = spawn(binary, args, { stdio: 'inherit' });

  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    process.on(signal, () => child.kill(signal));
  }

  child.on('exit', (code, signal) => {
    process.exit(signal ? 1 : (code ?? 0));
  });

  child.on('error', (err) => {
    process.stderr.write(`vaughan: failed to start ${binary}: ${err.message}\n`);
    process.exit(1);
  });
}

if (require.main === module) {
  main();
}

module.exports = { main };
