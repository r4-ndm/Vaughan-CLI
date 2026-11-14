#!/usr/bin/env node
'use strict';
// vaughan-mcp — MCP stdio server entrypoint shipped inside the `vaughan` launcher
// package. Uses the same locally installed platform binary (no runtime download).
// Supports `--version` / `-V` directly and otherwise execs `vaughan mcp <args...>`.

const { spawn } = require('child_process');
const { resolveBinary } = require('../lib/resolve');

function main() {
  const { binary, error } = resolveBinary();
  if (error) {
    process.stderr.write(`vaughan-mcp: ${error.message}\n`);
    process.exit(1);
  }

  const args = process.argv.slice(2);

  // `vaughan-mcp --version` / `-V` must succeed for humans checking their install,
  // rather than failing inside the `mcp` subcommand parser.
  let spawnArgs;
  if (args.includes('--version') || args.includes('-V')) {
    spawnArgs = ['--version'];
  } else {
    const forwarded = args.slice();
    if (process.env.VAUGHAN_PROFILE && !forwarded.includes('--profile')) {
      forwarded.push('--profile', process.env.VAUGHAN_PROFILE);
    }
    spawnArgs = ['mcp', ...forwarded];
  }

  const child = spawn(binary, spawnArgs, { stdio: 'inherit' });

  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    process.on(signal, () => child.kill(signal));
  }

  child.on('exit', (code, signal) => {
    process.exit(signal ? 1 : (code ?? 0));
  });

  child.on('error', (err) => {
    process.stderr.write(`vaughan-mcp: failed to start ${binary}: ${err.message}\n`);
    process.exit(1);
  });
}

if (require.main === module) {
  main();
}

module.exports = { main };
