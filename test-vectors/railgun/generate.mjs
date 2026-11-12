// Generates `vectors.json` — RAILGUN key-derivation parity vectors for
// `vaughan-railgun`, computed with the official engine
// (@railgun-community/engine). The Rust tests assert that railgun-rust,
// fed Vaughan's BIP-39 seed, derives exactly these 0zk addresses.
//
// Dev-only one-off: never a build or runtime dependency of Vaughan.
// The key-derivation module isn't in the package's `exports` map, so it is
// loaded by file path.
//
// Run:  npm install --ignore-scripts && npm run generate
// Output: vectors.json (committed; regenerate only when the engine pin changes)

import { createRequire } from 'node:module';
import { writeFileSync } from 'node:fs';

const require = createRequire(import.meta.url);
const { deriveNodes, WalletNode, encodeAddress } = require(
  './node_modules/@railgun-community/engine/dist/key-derivation/index.js',
);
const { version } = require('./node_modules/@railgun-community/engine/package.json');

// Same mnemonic as vaughan-core's account tests (BIP-39 "abandon … about").
const MNEMONIC =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';

const toHex = (bytes) => Buffer.from(bytes).toString('hex');

const accounts = [];
for (const index of [0, 1, 7]) {
  const nodes = deriveNodes(MNEMONIC, index);
  const spending = nodes.spending.getSpendingKeyPair();
  const nullifyingKey = await nodes.viewing.getNullifyingKey();
  const viewing = await nodes.viewing.getViewingKeyPair();
  const masterPublicKey = WalletNode.getMasterPublicKey(spending.pubkey, nullifyingKey);
  accounts.push({
    index,
    // All-chains address (no chain in AddressData), the Railway default.
    address: encodeAddress({ masterPublicKey, viewingPublicKey: viewing.pubkey }),
    masterPublicKey: masterPublicKey.toString(16).padStart(64, '0'),
    viewingPublicKey: toHex(viewing.pubkey),
  });
}

const vectors = { engine: `@railgun-community/engine@${version}`, mnemonic: MNEMONIC, accounts };
writeFileSync('vectors.json', `${JSON.stringify(vectors, null, 2)}\n`);
console.log(`wrote ${accounts.length} accounts`);
