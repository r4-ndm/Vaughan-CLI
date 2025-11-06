#!/bin/bash

# Dataset Collection for Vaughan Crush Model

echo "📚 Collecting training data for Vaughan Crush model..."

DATASET_DIR="training-data"
VAUGHAN_EXAMPLES="$DATASET_DIR/vaughan-examples.jsonl"
CAST_TEMPLATES="$DATASET_DIR/cast-templates.jsonl"

# Create dataset directory
mkdir -p "$DATASET_DIR"

echo "🎯 Generating Vaughan Crush specific examples..."

# Generate blockchain-specific examples
cat > "$VAUGHAN_EXAMPLES" << 'EOF'
{"prompt": "Check gas prices on sepolia", "response": "I'll check current gas prices on Sepolia testnet for you. ⛽\n\nCurrent gas price on Sepolia: 15 gwei\n💡 Recommendation: Use standard strategy for testnet transactions\n\nEstimated transaction costs:\n- Simple ETH transfer: ~$0 (free on testnet)\n- ERC20 token transfer: ~$0 (free on testnet)\n\nCast command: cast gas-price --rpc-url https://ethereum-sepolia.publicnode.com"}
{"prompt": "What's vitalik.eth's balance?", "response": "I'll check vitalik.eth's balance for you. 🔍\n\nConverting vitalik.eth → 0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045\n📊 Checking balance...\n\n💰 Balance: 1,234.567890 ETH\n📋 Address: 0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045\n🌐 Network: Mainnet\n\nCast command: cast balance vitalik.eth"}
{"prompt": "Send 0.1 ETH to vitalik.eth on sepolia", "response": "I'll help you send 0.1 ETH to vitalik.eth on Sepolia testnet. 🚀\n\n🔍 Recipient details:\n- ENS name: vitalik.eth\n- Address: 0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045\n- Network: Sepolia (testnet)\n- Amount: 0.1 ETH\n\n⛽ Gas estimation:\n- Current gas: 15 gwei\n- Estimated limit: 21,000 gas\n- Max cost: ~0.000315 ETH\n💰 Testnet cost: $0 (free gas!)\n\n⚠️ Security check: You're sending to a testnet address. This is safe for testing.\n\n✅ Ready to send transaction. Confirm?\n\nCast command: cast send 0.1 ETH --to vitalik.eth --rpc-url https://ethereum-sepolia.publicnode.com"}
{"prompt": "Check USDC allowance for Uniswap", "response": "I'll check your USDC allowance for Uniswap V3. 🔍\n\n📋 Contract details:\n- Token: USDC (0xA0b86a33E6444028b8624b2E5B7C8C3C8e8dE8)\n- Spender: Uniswap V3 Router (0xE592427A0AEce92De3Edee1F18E0157C05861564)\n📊 Checking allowance...\n\n💰 Current allowance: 500.00 USDC\n📋 Your wallet: [connect your wallet to check]\n🌐 Network: Mainnet\n\nIf allowance is insufficient:\nCast command: cast approve 0xE592427A0AEce92De3Edee1F18E0157C05861564 1000 USDC"}
{"prompt": "What's current gas price?", "response": "I'll check current gas prices for you. ⛽\n\n📊 Current Gas Market:\n- Slow: 20 gwei (saves money, slower confirmation)\n- Standard: 25 gwei (balanced speed/cost)\n- Fast: 30 gwei (quick confirmation, higher cost)\n\n💡 Recommendations:\n- For regular transfers: Use standard (25 gwei)\n- For urgent transactions: Use fast (30 gwei)\n- For batch operations: Use slow (20 gwei)\n\n🌐 Network: Ethereum Mainnet\n📋 1 confirmation typically takes 12-15 seconds with standard gas\n\nCast command: cast gas-price"}
{"prompt": "Deploy simple storage contract", "response": "I'll help you deploy a Simple Storage contract. 🚀\n\n📋 Contract details:\n- Type: Simple Storage (stores/retrieves uint256)\n- Size: ~200 bytes\n- Estimated gas: ~800,000\n💰 Deployment cost: ~$20-25 (depending on gas)\n\n📝 Solidity code:\n```solidity\n// SPDX-License-Identifier: MIT\npragma solidity ^0.8.19;\n\ncontract SimpleStorage {\n    uint256 private storedData;\n\n    function set(uint256 _data) public {\n        storedData = _data;\n    }\n\n    function get() public view returns (uint256) {\n        return storedData;\n    }\n}\n```\n\n⚠️ Before deploying:\n1. Choose network (recommend sepolia for testing)\n2. Check wallet balance for gas fees\n3. Verify contract code\n\n🚀 Deployment command (testnet):\ncast create \"<bytecode>\" --rpc-url https://ethereum-sepolia.publicnode.com\n\nWant to compile first? Use forge build SimpleStorage.sol"}
{"prompt": "Call balanceOf function on ERC20", "response": "I'll help you call the balanceOf function on an ERC20 token. 🔍\n\n📋 ERC20 balanceOf function:\n```solidity\nfunction balanceOf(address account) public view returns (uint256)\n```\n\n🌐 Format: cast call <contract> \"balanceOf(address)\" <address>\n\n📝 Examples:\n# Check USDC balance\ncast call 0xA0b86a33E6444028b8624b2E5B7C8C3C8e8dE8 \"balanceOf(address)\" 0x1234...\n\n# Check your WETH balance\ncast call 0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2 \"balanceOf(address)\" 0x1234...\n\n# Check token balance by ENS\ncast call 0xA0b86a33E6444028b8624b2E5B7C8C3C8e8dE8 \"balanceOf(address)\" vitalik.eth\n\n⚠️ Note: Returns value in wei (1 token = 10^18 wei)\n💡 Divide by 10^18 for human-readable amount"}
{"prompt": "Best gas strategy for NFT mint?", "response": "I'll help you choose the best gas strategy for NFT minting. ⛽🎨\n\n📊 NFT Mint Gas Analysis:\nNFT mints often compete in gas auctions, especially for popular drops.\n\n🎯 Recommended Strategies:\n\n**For Popular Drops (1-2 min before):**\n- Fast strategy: 35-50 gwei (avoid front-running)\n- Priority fee: 2-5 gwei (ensure miner inclusion)\n- Max fee: 100 gwei (safety buffer)\n\n**For Regular Mints (low competition):**\n- Standard strategy: 25-30 gwei\n- Priority fee: 1-2 gwei\n- Max fee: 50 gwei\n\n💡 Pro Tips:\n1. ⏰ Time your mint: Gas is lower 2-6 AM UTC\n2. 🧪 Test first: Always test on Goerli before mainnet\n3. 💰 Budget gas: Set max limit to avoid surprise costs\n4. 🚀 Flashbots: For high-value mints, consider flashbots\n\n📋 Cast command for mint (example):\ncast send <transaction> --gas-price 40000000000 --priority-fee 2000000000\n\nEstimated NFT mint cost: ~$5-15 depending on network congestion"}
{"prompt": "Simulate transaction before sending", "response": "I'll help you simulate the transaction before sending. 🔍⚡\n\n📋 Transaction Simulation Benefits:\n- ✅ Verify transaction will succeed\n- 💰 See exact gas costs\n- 🔍 Check for potential errors\n- 🛡️ Avoid failed transactions\n\n🚀 Simulation Process:\n\n**Method 1: Cast estimate (simple)**\n```bash\ncast estimate <to> <data> --rpc-url <network>\n```\n\n**Method 2: Eth_call simulation (comprehensive)**\n```bash\ncast call <to> <signature> <args> --from <sender> --rpc-url <network>\n```\n\n📊 Example - Transfer 1 ETH:\n```bash\n# Simulate the transfer\ncast call 0x1234... \"transfer(address,uint256)\" 0x5678... 1000000000000000000 --from 0x1234...\n\n# This will show if the transfer succeeds and the gas needed\n```\n\n🔍 What to look for in results:\n- ✅ Success: Transaction will work\n- ❌ Revert: Transaction will fail (why?)\n- ⛽ Gas limit: How much gas needed\n- 💰 Cost: Approximate transaction cost\n\n💡 Best Practice: Always simulate before mainnet transactions!\n🧪 Test first: Use Goerli/Sepolia for testing"}
EOF

echo "🎯 Generating Cast command template examples..."

# Generate Cast command specific examples
cat > "$CAST_TEMPLATES" << 'EOF'
{"prompt": "How to check ERC20 total supply?", "response": "I'll show you how to check ERC20 total supply using Cast. 📊\n\n🔍 ERC20 totalSupply() function:\n```solidity\nfunction totalSupply() public view returns (uint256)\n```\n\n📝 Cast command:\n```bash\ncast call <token_contract_address> \"totalSupply()\" --rpc-url <network>\n```\n\n📝 Examples:\n# USDC total supply\ncast call 0xA0b86a33E6444028b8624b2E5B7C8C3C8e8dE8 \"totalSupply()\" --rpc-url https://eth.llamarpc.com\n\n# WETH total supply\ncast call 0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2 \"totalSupply()\" --rpc-url https://eth.llamarpc.com\n\n# Testnet (Sepolia) total supply\ncast call 0xA0b86a33E6444028b8624b2E5B7C8C3C8e8dE8 \"totalSupply()\" --rpc-url https://ethereum-sepolia.publicnode.com\n\n💡 Remember: Returns wei (18 decimals), divide by 10^18 for human readable"}
{"prompt": "How to approve token spending?", "response": "I'll show you how to approve token spending using Cast. ✅\n\n🔍 ERC20 approve() function:\n```solidity\nfunction approve(address spender, uint256 amount) public returns (bool)\n```\n\n📝 Cast command:\n```bash\ncast send --to <token_contract> \"approve(address,uint256)\" <spender_address> <amount> --rpc-url <network>\n```\n\n📝 Examples:\n# Approve 1000 USDC to Uniswap V3 Router\ncast send --to 0xA0b86a33E6444028b8624b2E5B7C8C3C8e8dE8 \"approve(address,uint256)\" 0xE592427A0AEce92De3Edee1F18E0157C05861564 1000000000000 --rpc-url https://eth.llamarpc.com\n\n# Approve 1 WETH to 1inch\ncast send --to 0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2 \"approve(address,uint256)\" 0x111111125421cA6cB73539465b3e6a40B1CA798d 1000000000000000000 --rpc-url https://eth.llamarpc.com\n\n⚠️ Important considerations:\n- ✅ Amount in wei (multiply by 10^18 for tokens)\n- 🛡️ Approve only needed amounts (security)\n- 🧪 Test on testnet first\n- 📊 Check current allowance after approval"}
{"prompt": "How to decode transaction data?", "response": "I'll show you how to decode transaction data using Cast. 🔍📋\n\n📝 Cast decode command:\n```bash\ncast 4byte-decode --input-data <tx_data>\ncast 4byte --select <function_signature>\ncast calldata-decode --calldata <calldata> --abi <abi_file>\n```\n\n📝 Examples:\n\n# Decode function signature\ncast 4byte-decode --input-data 0xa9059cbb0000000000000000000000000000000000000000000000000000000000000000a\n\n# Get function details\ncast 4byte --select \"transfer(address,uint256)\"\n\n# Decode with ABI\ncast calldata-decode --calldata \"0xa9059cbb0000000000000000000000000000000000000000000000000000000000000000a\" --abi erc20.json\n\n📊 What you'll learn:\n- ✅ Function name (e.g., transfer)\n- 📋 Parameter types (address, uint256)\n- 🔢 Decoded arguments (recipient, amount)\n- 📚 Source contract if available\n\n💡 Pro Tips:\n- 🔍 Use 4byte directory to look up unknown signatures\n- 📋 Save ABI files for common contracts (ERC20, Uniswap)\n- 🧪 Test decoding with known transactions first"}
EOF

echo "📊 Dataset collection complete!"

# Count examples
VAUGHAN_COUNT=$(wc -l < "$VAUGHAN_EXAMPLES")
CAST_COUNT=$(wc -l < "$CAST_TEMPLATES")
TOTAL_COUNT=$((VAUGHAN_COUNT + CAST_COUNT))

echo "📋 Dataset Statistics:"
echo "   Vaughan examples: $VAUGHAN_COUNT"
echo "   Cast templates: $CAST_COUNT"  
echo "   Total examples: $TOTAL_COUNT"
echo ""
echo "📁 Files created:"
echo "   $VAUGHAN_EXAMPLES"
echo "   $CAST_TEMPLATES"
echo ""
echo "🚀 Ready for model training!"
echo ""
echo "🎯 Next steps:"
echo "1. Add more examples to build larger dataset"
echo "2. Convert to fine-tuning format for your chosen framework"
echo "3. Train using: ollama fine-tune or external ML framework"
echo "4. Validate model performance on blockchain tasks"
echo ""
echo "💡 Tip: Focus on common blockchain queries for best results!"