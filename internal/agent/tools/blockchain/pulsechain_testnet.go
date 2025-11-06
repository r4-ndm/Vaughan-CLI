package blockchain

import (
	"context"
	_ "embed"
	"fmt"
	"html/template"
	"strings"

	"github.com/r4v3n/vaughan-cli/internal/fantasy"
	"github.com/r4v3n/vaughan-cli/internal/permission"
	"github.com/r4v3n/vaughan-cli/internal/shell"
)

// PulseChainTestnetTool represents PulseChain Testnet V4 support
type PulseChainTestnetTool struct {
	shellRunner shell.ShellRunner
}

// PulseChainTestnetResponse represents testnet network information
type PulseChainTestnetResponse struct {
	Network     string `json:"network"`
	ChainID     int    `json:"chain_id"`
	RPCUrl      string `json:"rpc_url"`
	GasToken    string `json:"gas_token"`
	BlockTime   int    `json:"block_time"`
	Explorer    string `json:"explorer"`
	Faucet      string `json:"faucet"`
	Type        string `json:"type"`
	Features    []string `json:"features"`
	Benefits    []string `json:"benefits"`
}

//go:embed pulsechain_testnet.tpl
var pulsechainTestnetTemplate string

// PulseChainTestnetName name of PulseChain Testnet tool
const PulseChainTestnetName = "pulsechain_testnet"

// NewPulseChainTestnetTool creates a new PulseChain Testnet tool
func NewPulseChainTestnetTool(shellRunner shell.ShellRunner) fantasy.AgentTool {
	return fantasy.NewAgentToolWithAllParams(
		PulseChainTestnetName,
		"PulseChain Testnet V4 Support - Free testing environment with tPLS token",
		"Access PulseChain Testnet V4 information, get testnet PLS from faucet, practice transactions safely. PulseChain Testnet V4 offers free gas with tPLS token, 2-second blocks, and full EVM compatibility for safe testing before mainnet deployment.",
		"Use this tool when:\n- User wants to test PulseChain V4 functionality\n- Need testnet tPLS from faucet\n- Want to practice transactions safely\n- Learning PulseChain development\n- Testing smart contracts before deployment\n- Comparing testnet vs mainnet features",
		func(ctx context.Context, toolInput *fantasy.ToolInput) (*fantasy.ToolResult, error) {
			return PulseChainTestnetTool(ctx, toolInput, shellRunner)
		},
	)
}

// PulseChainTestnetTool implements PulseChain Testnet V4 tool
func PulseChainTestnetTool(ctx context.Context, toolInput *fantasy.ToolInput, shellRunner shell.ShellRunner) (*fantasy.ToolResult, error) {
	tool := PulseChainTestnetTool{
		shellRunner: shellRunner,
	}

	type ToolInput struct {
		Action      string `json:"action"`
		GetFaucet  bool   `json:"get_faucet"`
		ShowSetup   bool   `json:"show_setup"`
		NetworkType string `json:"network_type"`
	}

	var input ToolInput
	if err := toolInput.UnmarshalParameters(&input); err != nil {
		return fantasy.NewToolResult(fantasy.ToolResultTypeError, "Invalid input parameters"), nil
	}

	switch input.Action {
	case "faucet":
		return tool.getTestnetFaucet(ctx)
	case "setup":
		return tool.setupTestnet(ctx, input.ShowSetup)
	case "compare":
		return tool.compareTestnetVsMainnet(ctx)
	case "gas_price":
		return tool.checkTestnetGasPrice(ctx)
	case "transaction":
		return tool.prepareTestnetTransaction(ctx, input.NetworkType)
	case "benefits":
		return tool.testnetBenefits(ctx)
	default:
		return tool.testnetOverview(ctx)
	}
}

// getTestnetFaucet provides faucet information
func (tool *PulseChainTestnetTool) getTestnetFaucet(ctx context.Context) (*fantasy.ToolResult, error) {
	var output strings.Builder
	output.WriteString("🧪 PulseChain Testnet V4 - tPLS Faucet\n\n")
	
	output.WriteString("💧 Testnet PLS (tPLS) Faucet:\n")
	output.WriteString("🔗 URL: https://faucet.pulsechain.com\n")
	output.WriteString("💱 Token: tPLS (testnet PulseChain)\n")
	output.WriteString("⏱️  Refill: Every 24 hours\n")
	output.WriteString("📊 Amount: Typically 100 tPLS per request\n")
	output.WriteString("✅ Free: No cost for testnet gas\n\n")
	
	output.WriteString("🔧 How to Get tPLS:\n")
	output.WriteString("1. 📱 Visit: https://faucet.pulsechain.com\n")
	output.WriteString("2. 🔗 Connect wallet (MetaMask, etc.)\n")
	output.WriteString("3. 🌐 Switch to PulseChain Testnet (Chain ID: 943)\n")
	output.WriteString("4. 💧 Click 'Get tPLS' button\n")
	output.WriteString("5. ✅ tPLS appears in wallet\n\n")
	
	output.WriteString("🛠️  Wallet Setup for Testnet:\n")
	output.WriteString("• Network Name: PulseChain Testnet V4\n")
	output.WriteString("• New RPC URL: https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Chain ID: 943 (0x3AF)\n")
	output.WriteString("• Currency Symbol: tPLS\n\n")
	
	output.WriteString("💻 Cast Commands with Testnet:\n")
	output.WriteString("• Balance: cast balance 0x123... --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Gas price: cast gas-price --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Send: cast send --to 0x456... --value 100tPLS --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Blocks: cast block latest --rpc-url https://testnet-rpc.pulsechain.com\n\n")
	
	output.WriteString("🤖 Vaughan Crush Testnet Integration:\n")
	output.WriteString("• Auto-detects testnet queries\n")
	output.WriteString("• Suggests testnet for new users\n")
	output.WriteString("• Provides faucet guidance\n")
	output.WriteString("• Shows tPLS gas costs (free!)\n")
	output.WriteString("• Links to testnet explorer\n")
	
	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, output.String()), nil
}

// setupTestnet provides testnet setup instructions
func (tool *PulseChainTestnetTool) setupTestnet(ctx context.Context, showSetup bool) (*fantasy.ToolResult, error) {
	var output strings.Builder
	output.WriteString("🧪 PulseChain Testnet V4 Setup Guide\n\n")
	
	output.WriteString("🌐 Testnet Network Details:\n")
	output.WriteString("• Name: PulseChain Testnet V4\n")
	output.WriteString("• Chain ID: 943 (0x3AF)\n")
	output.WriteString("• RPC URL: https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Explorer: https://testnet-scan.pulsechain.com\n")
	output.WriteString("• Gas Token: tPLS (testnet)\n")
	output.WriteString("• Block Time: ~2 seconds\n")
	output.WriteString("• Type: Testnet (free testing)\n\n")
	
	output.WriteString("📱 MetaMask Setup:\n")
	output.WriteString("1. 🌐 Open MetaMask\n")
	output.WriteString("2. 🔗 Click network dropdown\n")
	output.WriteString("3. ➕ 'Add Network' or 'Add RPC'\n")
	output.WriteString("4. 📝 Enter details:\n")
	output.WriteString("   Network Name: PulseChain Testnet V4\n")
	output.WriteString("   New RPC URL: https://testnet-rpc.pulsechain.com\n")
	output.WriteString("   Chain ID: 943\n")
	output.WriteString("   Currency Symbol: tPLS\n")
	output.WriteString("5. ✅ Save and switch to PulseChain Testnet V4\n\n")
	
	output.WriteString("🛠️  Alternative Wallet Setup:\n")
	output.WriteString("• WalletConnect: Add testnet RPC\n")
	output.WriteString("• Frame: Custom EVM chain\n")
	output.WriteString("• Rabby: Import chain with Chain ID 943\n")
	output.WriteString("• Trust Wallet: Add custom EVM\n\n")
	
	output.WriteString("💧 Get Testnet PLS (tPLS):\n")
	output.WriteString("1. 📱 Visit: https://faucet.pulsechain.com\n")
	output.WriteString("2. 🔗 Connect your testnet wallet\n")
	output.WriteString("3. 💧 Request tPLS (usually 100 per request)\n")
	output.WriteString("4. ⏰ Wait 24 hours for next request\n")
	output.WriteString("5. ✅ tPLS ready for transactions\n\n")
	
	output.WriteString("🧪 Test First Transaction:\n")
	output.WriteString("1. 🔍 Get test address: 0x123...\n")
	output.WriteString("2. 💧 Ensure tPLS balance: >0\n")
	output.WriteString("3. 📊 Send test transaction:\n")
	output.WriteString("   cast send --to 0x456... --value 1tPLS --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("4. 🔍 Verify on: https://testnet-scan.pulsechain.com\n")
	output.WriteString("5. ✅ Success! Ready for development\n\n")
	
	output.WriteString("🤖 Vaughan Crush Testnet Features:\n")
	output.WriteString("• Auto-suggests testnet for new users\n")
	output.WriteString("• Generates testnet Cast commands\n")
	output.WriteString("• Provides faucet links and guidance\n")
	output.WriteString("• Shows tPLS gas costs (free!)\n")
	output.WriteString("• Links to testnet explorer\n")
	output.WriteString("• Safe learning environment\n")
	
	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, output.String()), nil
}

// compareTestnetVsMainnet compares testnet vs mainnet
func (tool *PulseChainTestnetTool) compareTestnetVsMainnet(ctx context.Context) (*fantasy.ToolResult, error) {
	var output strings.Builder
	output.WriteString("🧪 PulseChain Testnet V4 vs Mainnet V4\n\n")
	
	output.WriteString("📊 Network Comparison:\n")
	output.WriteString("┌─────────────────┬────────────┬─────────────────┬─────────────┐\n")
	output.WriteString("│ Feature        │ Testnet V4  │ Mainnet V4     │ Benefits    │\n")
	output.WriteString("├─────────────────┼────────────┼─────────────────┼─────────────┤\n")
	output.WriteString("│ Chain ID       │ 943         │ 369            │ Unique IDs  │\n")
	output.WriteString("│ RPC URL        │ testnet-rpc │ rpc.pulsechain  │ Separate    │\n")
	output.WriteString("│ Gas Token      │ tPLS        │ PLS            │ Free/Test   │\n")
	output.WriteString("│ Gas Cost       │ Free        │ Real cost      │ Safe testing│\n")
	output.WriteString("│ Block Time     │ 2 seconds   │ 2 seconds      │ Fast        │\n")
	output.WriteString("│ Explorer       │ testnet-scan│ scan.pulsechain│ Separate    │\n")
	output.WriteString("│ Faucet         │ Available   │ No faucet      │ Free tokens│\n")
	output.WriteString("└─────────────────┴────────────┴─────────────────┴─────────────┘\n\n")
	
	output.WriteString("🎯 When to Use Each:\n")
	output.WriteString("🧪 Use Testnet V4 For:\n")
	output.WriteString("• Learning PulseChain development\n")
	output.WriteString("• Testing smart contracts\n")
	output.WriteString("• Practicing transactions\n")
	output.WriteString("• Debugging applications\n")
	output.WriteString("• No-cost experimentation\n")
	output.WriteString("• Educational purposes\n")
	output.WriteString("• Security testing\n\n")
	
	output.WriteString("⚡ Use Mainnet V4 For:\n")
	output.WriteString("• Production applications\n")
	output.WriteString("• Real value transfers\n")
	output.WriteString("• DeFi protocol interactions\n")
	output.WriteString("• NFT minting and trading\n")
	output.WriteString("• Business transactions\n")
	output.WriteString("• Live deployment\n\n")
	
	output.WriteString("🚀 Recommended Workflow:\n")
	output.WriteString("1. 🧪 Develop on testnet (free tPLS)\n")
	output.WriteString("2. 🔍 Test thoroughly (no cost)\n")
	output.WriteString("3. ✅ Verify functionality works\n")
	output.WriteString("4. ⚡ Deploy to mainnet (real PLS)\n")
	output.WriteString("5. 📊 Monitor production transactions\n")
	
	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, output.String()), nil
}

// checkTestnetGasPrice checks testnet gas prices
func (tool *PulseChainTestnetTool) checkTestnetGasPrice(ctx context.Context) (*fantasy.ToolResult, error) {
	var output strings.Builder
	output.WriteString("🧪 PulseChain Testnet V4 Gas Prices\n\n")
	
	output.WriteString("💱 Testnet Gas Market (tPLS):\n")
	output.WriteString("• Slow: 2 tPLS (FREE on testnet!)\n")
	output.WriteString("• Standard: 3 tPLS (FREE on testnet!)\n")
	output.WriteString("• Fast: 5 tPLS (FREE on testnet!)\n\n")
	
	output.WriteString("💡 Testnet Benefits:\n")
	output.WriteString("• All gas costs are FREE!\n")
	output.WriteString("• tPLS from faucet covers everything\n")
	output.WriteString("• No risk of losing real money\n")
	output.WriteString("• 2-second block confirmations\n")
	output.WriteString("• EVM compatible tools work\n\n")
	
	output.WriteString("💻 Cast Commands for Testnet:\n")
	output.WriteString("• Gas price: cast gas-price --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Block info: cast block latest --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Chain ID: cast chain-id --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Send: cast send --to 0x456... --value 1tPLS --rpc-url https://testnet-rpc.pulsechain.com\n\n")
	
	output.WriteString("🤖 Vaughan Crush Testnet Integration:\n")
	output.WriteString("• Auto-generates testnet Cast commands\n")
	output.WriteString("• Shows FREE gas costs (tPLS)\n")
	output.WriteString("• Provides faucet links\n")
	output.WriteString("• Links to testnet explorer\n")
	output.WriteString("• Suggests testnet for learning\n")
	output.WriteString("• Safe, cost-free environment\n")
	
	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, output.String()), nil
}

// prepareTestnetTransaction prepares testnet transactions
func (tool *PulseChainTestnetTool) prepareTestnetTransaction(ctx context.Context, networkType string) (*fantasy.ToolResult, error) {
	var output strings.Builder
	output.WriteString("🧪 PulseChain Testnet V4 Transaction Preparation\n\n")
	
	output.WriteString("⚡ Testnet Transaction Benefits:\n")
	output.WriteString("• FREE gas costs (tPLS from faucet)\n")
	output.WriteString("• Fast 2-second confirmations\n")
	output.WriteString("• Safe learning environment\n")
	output.WriteString("• EVM compatible tools\n")
	output.WriteString("• Testnet explorer verification\n")
	output.WriteString("• No risk to real funds\n\n")
	
	output.WriteString("📋 Testnet Transaction Types:\n")
	output.WriteString("• tPLS Transfers: Basic token transfers\n")
	output.WriteString("• tETH Transfers: Testnet ETH transfers\n")
	output.WriteString("• Smart Contract Calls: Contract interactions\n")
	output.WriteString("• NFT Minting: Test NFT creation\n")
	output.WriteString("• DeFi Testing: Protocol interactions\n")
	output.WriteString("• Bridge Testing: Cross-chain practice\n\n")
	
	output.WriteString("💻 Cast Testnet Commands:\n")
	output.WriteString("• Send tPLS: cast send --to 0x456... --value 100tPLS --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Send tETH: cast send --to 0x456... --value 1tether --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Contract call: cast call 0x789... \"balanceOf(address)\" 0x123... --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("• Estimate gas: cast estimate 0x456... --value 1tether --rpc-url https://testnet-rpc.pulsechain.com\n\n")
	
	output.WriteString("🧪 Safe Testing Workflow:\n")
	output.WriteString("1. 💧 Get tPLS from faucet: https://faucet.pulsechain.com\n")
	output.WriteString("2. 🔍 Verify tPLS balance: cast balance --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("3. 📊 Send test transaction: cast send --to 0x456... --value 1tPLS --rpc-url https://testnet-rpc.pulsechain.com\n")
	output.WriteString("4. 🔍 Verify on explorer: https://testnet-scan.pulsechain.com\n")
	output.WriteString("5. ✅ Transaction confirmed (2 seconds!)\n")
	output.WriteString("6. 🧪 Repeat for testing: No gas costs!\n\n")
	
	output.WriteString("🤖 Vaughan Crush Testnet Features:\n")
	output.WriteString("• Auto-detects testnet queries\n")
	output.WriteString("• Generates testnet Cast commands\n")
	output.WriteString("• Shows FREE gas (tPLS)\n")
	output.WriteString("• Provides faucet guidance\n")
	output.WriteString("• Links to testnet explorer\n")
	output.WriteString("• Safe learning environment\n")
	output.WriteString("• No-cost experimentation\n")
	
	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, output.String()), nil
}

// testnetBenefits explains testnet advantages
func (tool *PulseChainTestnetTool) testnetBenefits(ctx context.Context) (*fantasy.ToolResult, error) {
	var output strings.Builder
	output.WriteString("🧪 PulseChain Testnet V4 - Benefits & Features\n\n")
	
	output.WriteString("💰 Cost Benefits:\n")
	output.WriteString("• FREE gas: tPLS from faucet covers all costs\n")
	output.WriteString("• No real money: Test tPLS has no value\n")
	output.WriteString("• Unlimited testing: Get more tPLS every 24 hours\n")
	output.WriteString("• Risk-free: No fear of losing real funds\n")
	output.WriteString("• Educational: Perfect learning environment\n\n")
	
	output.WriteString("⚡ Performance Benefits:\n")
	output.WriteString("• Fast blocks: 2-second confirmations\n")
	output.WriteString("• EVM compatible: Same tools as mainnet\n")
	output.WriteString("• Stable network: Reliable for testing\n")
	output.WriteString("• Same features: Full mainnet functionality\n")
	output.WriteString("• Real conditions: Actual production behavior\n")
	output.WriteString("• Debugging tools: Explorer and Cast support\n\n")
	
	output.WriteString("🛡️ Security Benefits:\n")
	output.WriteString("• Isolated environment: No impact on mainnet\n")
	output.WriteString("• Test everything: Smart contracts, transfers, etc.\n")
	output.WriteString("• Practice security: Test exploits safely\n")
	output.WriteString("• Learning curve: No pressure on real funds\n")
	output.WriteString("• Mistake tolerance: Errors cost nothing\n")
	output.WriteString("• Experiment freely: Try new ideas safely\n\n")
	
	output.WriteString("🎯 Learning Benefits:\n")
	output.WriteString("• Real development: Actual EVM environment\n")
	output.WriteString("• Tool practice: Cast, Hardhat, etc.\n")
	output.WriteString("• Blockchain interaction: Real transaction flow\n")
	output.WriteString("• Smart contract testing: Deploy and test\n")
	output.WriteString("• DeFi understanding: Protocol testing\n")
	output.WriteString("• Ecosystem familiarization: PulseChain features\n")
	output.WriteString("• Production readiness: Skills transfer to mainnet\n\n")
	
	output.WriteString("🚀 Vaughan Crush Testnet Integration:\n")
	output.WriteString("• AI assistance for testnet queries\n")
	output.WriteString("• Automatic testnet detection\n")
	output.WriteString("• Faucet guidance and links\n")
	output.WriteString("• Free gas cost awareness\n")
	output.WriteString("• Testnet Cast command generation\n")
	output.WriteString("• Safe learning recommendations\n")
	output.WriteString("• Explorer integration for verification\n")
	
	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, output.String()), nil
}

// testnetOverview provides general testnet information
func (tool *PulseChainTestnetTool) testnetOverview(ctx context.Context) (*fantasy.ToolResult, error) {
	testnetInfo := PulseChainTestnetResponse{
		Network:   "PulseChain Testnet V4",
		ChainID:   943,
		RPCUrl:    "https://testnet-rpc.pulsechain.com",
		GasToken:  "tPLS",
		BlockTime:  2,
		Explorer:  "https://testnet-scan.pulsechain.com",
		Faucet:    "https://faucet.pulsechain.com",
		Type:      "testnet",
		Features:  []string{"Free Gas", "Fast Blocks", "EVM Compatible", "Faucet Available", "Safe Testing"},
		Benefits:  []string{"Cost-free testing", "No real money risk", "2-second blocks", "Full EVM compatibility", "Unlimited learning"},
	}
	
	tmpl, err := template.New("pulsechain_testnet").Parse(pulsechainTestnetTemplate)
	if err != nil {
		return fantasy.NewToolResult(fantasy.ToolResultTypeError, fmt.Sprintf("Template error: %v", err)), nil
	}
	
	var output strings.Builder
	err = tmpl.Execute(&output, map[string]interface{}{
		"Response": "PulseChain Testnet V4 - Free Testing Environment",
		"Network": testnetInfo,
		"Command":  "",
		"Error":    "",
	})
	
	if err != nil {
		return fantasy.NewToolResult(fantasy.ToolResultTypeError, fmt.Sprintf("Template execution error: %v", err)), nil
	}
	
	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, output.String()), nil
}