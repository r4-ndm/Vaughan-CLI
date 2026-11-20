Real risks to know about before going further:

Upstream Kohaku itself is explicitly unaudited and pre-production — you're building on a moving, unstable foundation, and Railgun's own Rust port inside kohaku is new enough to have open correctness bugs.
Specifically: there's an open GitHub issue on ethereum/kohaku reporting that RAILGUN keys derived via Kohaku's plugin flow use standard BIP-32 secp256k1 derivation, while the canonical RAILGUN engine's reference implementation uses a different "babyjubjub seed" derivation tree for spending and viewing keys — producing unrelated keys from the same mnemonic and making wallets incompatible with the wider RAILGUN ecosystem. If kohaku-railgun inherits this, funds shielded through your wallet could be unrecoverable or incompatible elsewhere. Worth tracking that issue closely before wiring up shield/unshield in kohaku-cli. 
GitHub
crates.io naming collision is already handled correctly in your README (kohaku is taken by an unrelated tokenizer) — good catch, just flagging you got that right.
It's very early (12 commits, most crates scaffolding/planned) — the real test is whether kohaku-railgun actually compiles against upstream once you start that git dependency; upstream's Rust crates may not be structured for external consumption yet (internal APIs, no semver stability).
