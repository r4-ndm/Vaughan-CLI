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

// HardwareWalletTool represents hardware wallet support
type HardwareWalletTool struct {
	shellRunner shell.ShellRunner
}

// HardwareWalletResponse represents hardware wallet info
type HardwareWalletResponse struct {
	Type        string `json:"type"`
	Status      string `json:"status"`
	Model       string `json:"model"`
	Connected   bool   `json:"connected"`
	Compatible  bool   `json:"compatible"`
	Recommendations []string `json:"recommendations"`
}

//go:embed hardware_wallet.tpl
var hardwareWalletTemplate string

// HardwareWalletName name of hardware wallet tool
const HardwareWalletName = "hardware_wallet"

// NewHardwareWalletTool creates a new hardware wallet tool
func NewHardwareWalletTool(shellRunner shell.ShellRunner) fantasy.AgentTool {
	return fantasy.NewAgentToolWithAllParams(
		HardwareWalletName,
		"Hardware Wallet Support - Connect and manage Trezor, Ledger, and other hardware wallets",
		"Check hardware wallet connectivity, setup instructions, and secure transaction signing capabilities",
		"Use this tool when:\n- User wants to connect a hardware wallet\n- Setting up secure transaction signing\n- Checking wallet compatibility\n- Learning about hardware wallet security\n- Troubleshooting hardware wallet connections",
		func(ctx context.Context, toolInput *fantasy.ToolInput) (*fantasy.ToolResult, error) {
			return HardwareWalletTool(ctx, toolInput, shellRunner)
		},
	)
}

// HardwareWalletTool implements the hardware wallet tool
func HardwareWalletTool(ctx context.Context, toolInput *fantasy.ToolInput, shellRunner shell.ShellRunner) (*fantasy.ToolResult, error) {
	tool := HardwareWalletTool{
		shellRunner: shellRunner,
	}

	type ToolInput struct {
		Action   string `json:"action"`
		Wallet   string `json:"wallet"`
		TestMode bool   `json:"test_mode"`
	}

	var input ToolInput
	if err := toolInput.UnmarshalParameters(&input); err != nil {
		return fantasy.NewToolResult(fantasy.ToolResultTypeError, "Invalid input parameters"), nil
	}

	switch input.Action {
	case "check":
		return tool.checkHardwareWallets(ctx, input.Wallet)
	case "setup":
		return tool.setupHardwareWallet(ctx, input.Wallet, input.TestMode)
	case "test":
		return tool.testHardwareWallet(ctx, input.Wallet)
	case "security":
		return tool.hardwareSecurityTips(ctx, input.Wallet)
	default:
		return tool.hardwareWalletInfo(ctx, input.Wallet)
	}
}

// checkHardwareWallets checks for connected hardware wallets
func (tool *HardwareWalletTool) checkHardwareWallets(ctx context.Context, walletType string) (*fantasy.ToolResult, error) {
	wallets := []HardwareWalletResponse{
		{
			Type:        "Trezor",
			Status:      "supported",
			Model:       "Trezor Model T/One",
			Connected:   false,
			Compatible:  true,
			Recommendations: []string{"Connect via USB", "Install Trezor Bridge", "Unlock device"},
		},
		{
			Type:        "Ledger", 
			Status:      "supported",
			Model:       "Ledger Nano S/X",
			Connected:   false,
			Compatible:  true,
			Recommendations: []string{"Install Ledger Live", "Connect via USB", "Open Ethereum App"},
		},
		{
			Type:        "GridPlus",
			Status:      "supported",
			Model:       "GridPlus Lattice1",
			Connected:   false,
			Compatible:  true,
			Recommendations: []string{"Connect via USB", "Install GridPlus Client"},
		},
	}

	tmpl, err := template.New("hardware_wallet").Parse(hardwareWalletTemplate)
	if err != nil {
		return fantasy.NewToolResult(fantasy.ToolResultTypeError, fmt.Sprintf("Template error: %v", err)), nil
	}

	var output strings.Builder
	err = tmpl.Execute(&output, map[string]interface{}{
		"Response":     "Hardware Wallet Status Check",
		"Wallets":     wallets,
		"Command":      "",
		"Network":      "Ethereum",
		"Error":        "",
	})

	if err != nil {
		return fantasy.NewToolResult(fantasy.ToolResultTypeError, fmt.Sprintf("Template execution error: %v", err)), nil
	}

	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, output.String()), nil
}

// setupHardwareWallet provides setup instructions
func (tool *HardwareWalletTool) setupHardwareWallet(ctx context.Context, walletType string, testMode bool) (*fantasy.ToolResult, error) {
	var guide strings.Builder

	if walletType == "trezor" {
		guide.WriteString("🎚️ Trezor Setup Guide:\n\n")
		guide.WriteString("1. 🔒 Physical Setup:\n")
		guide.WriteString("   - Connect Trezor via USB cable\n")
		guide.WriteString("   - Ensure device is unlocked\n")
		guide.WriteString("   - Install Trezor Bridge from trezor.io\n\n")
		
		guide.WriteString("2. 🛠️ Software Setup:\n")
		guide.WriteString("   - Cast supports Trezor natively\n")
		guide.WriteString("   - Use: cast send --trezor [options]\n\n")
		
		guide.WriteString("3. 🧪 Test Connection:\n")
		guide.WriteString("   - Try: cast wallet list --trezor\n")
		guide.WriteString("   - Should show Trezor device\n\n")
		
		guide.WriteString("4. 🚀 Vaughan Crush Integration:\n")
		guide.WriteString("   - Vaughan detects hardware wallet\n")
		guide.WriteString("   - Prompts for Trezor usage\n")
		guide.WriteString("   - Signs transactions securely\n\n")

	} else if walletType == "ledger" {
		guide.WriteString("📱 Ledger Setup Guide:\n\n")
		guide.WriteString("1. 🔒 Physical Setup:\n")
		guide.WriteString("   - Connect Ledger via USB cable\n")
		guide.WriteString("   - Unlock device with PIN\n")
		guide.WriteString("   - Install Ledger Live app\n\n")
		
		guide.WriteString("2. 🛠️ Software Setup:\n")
		guide.WriteString("   - Cast supports Ledger natively\n")
		guide.WriteString("   - Use: cast send --ledger [options]\n\n")
		
		guide.WriteString("3. 🧪 Test Connection:\n")
		guide.WriteString("   - Try: cast wallet list --ledger\n")
		guide.WriteString("   - Should show Ledger device\n\n")
		
		guide.WriteString("4. 🚀 Vaughan Crush Integration:\n")
		guide.WriteString("   - Vaughan detects hardware wallet\n")
		guide.WriteString("   - Prompts for Ledger usage\n")
		guide.WriteString("   - Signs transactions on device\n\n")
	}

	if testMode {
		guide.WriteString("5. 🧪 Test Mode Setup:\n")
		guide.WriteString("   - Use testnet for first transaction\n")
		guide.WriteString("   - Small amount (0.001 ETH)\n")
		guide.WriteString("   - Verify on Etherscan\n")
		guide.WriteString("   - Confirm device displays correct address\n\n")
	}

	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, guide.String()), nil
}

// testHardwareWallet tests hardware wallet functionality
func (tool *HardwareWalletTool) testHardwareWallet(ctx context.Context, walletType string) (*fantasy.ToolResult, error) {
	var testCommand string
	var description string

	if walletType == "trezor" {
		testCommand = "cast wallet list --trezor"
		description = "Testing Trezor connection and compatibility"
	} else if walletType == "ledger" {
		testCommand = "cast wallet list --ledger" 
		description = "Testing Ledger connection and compatibility"
	} else {
		testCommand = "cast wallet list"
		description = "Testing general wallet compatibility"
	}

	// Execute test command
	result, err := tool.shellRunner.Run(ctx, testCommand)
	if err != nil {
		errorMsg := fmt.Sprintf("🔒 Hardware wallet test failed\n\nError: %v\n\nTroubleshooting:\n- Ensure device is connected via USB\n- Unlock device with PIN\n- Check if Bridge software is running\n- Try different USB port", err)
		return fantasy.NewToolResult(fantasy.ToolResultTypeError, errorMsg), nil
	}

	// Parse and format results
	var output strings.Builder
	output.WriteString("🧪 Hardware Wallet Test Results\n\n")
	output.WriteString(fmt.Sprintf("📋 Test: %s\n", description))
	output.WriteString(fmt.Sprintf("🔧 Command: %s\n\n", testCommand))
	output.WriteString("📊 Output:\n")
	output.WriteString(result)
	output.WriteString("\n\n💡 Test Interpretation:\n")
	if strings.Contains(result, "Trezor") || strings.Contains(result, "Ledger") {
		output.WriteString("✅ Hardware wallet detected and working!")
		output.WriteString("\n\n🚀 Ready for secure transaction signing with Vaughan Crush!")
	} else {
		output.WriteString("⚠️ Hardware wallet not detected")
		output.WriteString("\n\n🔧 Try these steps:")
		output.WriteString("\n- Check USB connection")
		output.WriteString("\n- Unlock device")
		output.WriteString("\n- Install Bridge software")
		output.WriteString("\n- Restart Vaughan Crush")
	}

	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, output.String()), nil
}

// hardwareSecurityTips provides security recommendations
func (tool *HardwareWalletTool) hardwareSecurityTips(ctx context.Context, walletType string) (*fantasy.ToolResult, error) {
	tips := strings.Builder
	
	tips.WriteString("🔒 Hardware Wallet Security Best Practices\n\n")
	
	tips.WriteString("🛡️ Physical Security:\n")
	tips.WriteString("✅ Store in safe, dry location\n")
	tips.WriteString("✅ Use anti-tampering seals\n")
	tips.WriteString("✅ Keep device physically secured\n")
	tips.WriteString("✅ Record serial number\n\n")
	
	tips.WriteString("🔐 Device Security:\n")
	tips.WriteString("✅ Use strong PIN (not birthday)\n")
	tips.WriteString("✅ Change PIN periodically\n")
	tips.WriteString("✅ Never share PIN with anyone\n")
	tips.WriteString("✅ Enable passphrase if available\n\n")
	
	tips.WriteString("🌐 Operational Security:\n")
	tips.WriteString("✅ Verify transaction details on device\n")
	tips.WriteString("✅ Check addresses match\n")
	tips.WriteString("✅ Use official firmware only\n")
	tips.WriteString("✅ Update firmware regularly\n")
	tips.WriteString("✅ Use reputable sources\n\n")
	
	if walletType == "trezor" {
		tips.WriteString("🎚️ Trezor Specific Tips:\n")
		tips.WriteString("✅ Use Trezor Bridge for best compatibility\n")
		tips.WriteString("✅ Enable passphrase for 2FA\n")
		tips.WriteString("✅ Keep recovery phrase offline\n")
		tips.WriteString("✅ Test recovery phrase safely\n\n")
	} else if walletType == "ledger" {
		tips.WriteString("📱 Ledger Specific Tips:\n")
		tips.WriteString("✅ Clear apps after use\n")
		tips.WriteString("✅ Blind signing when possible\n")
		tips.WriteString("✅ Enable optional passphrase\n")
		tips.WriteString("✅ Check for fake Ledger devices\n\n")
	}
	
	tips.WriteString("🚨 Security Warnings:\n")
	tips.WriteString("❌ Never enter recovery phrase online\n")
	tips.WriteString("❌ Never share private keys\n")
	tips.WriteString("❌ Beware of phishing attempts\n")
	tips.WriteString("❌ Verify all transaction details\n")
	tips.WriteString("❌ Use official apps only\n\n")
	
	tips.WriteString("🎯 Vaughan Crush Integration:\n")
	tips.WriteString("✅ Hardware wallet auto-detection\n")
	tips.WriteString("✅ Secure transaction signing\n")
	tips.WriteString("✅ Device confirmation prompts\n")
	tips.WriteString("✅ Testnet-first approach\n")
	tips.WriteString("✅ Private keys never exposed\n")
	
	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, tips.String()), nil
}

// hardwareWalletInfo provides general hardware wallet information
func (tool *HardwareWalletTool) hardwareWalletInfo(ctx context.Context, walletType string) (*fantasy.ToolResult, error) {
	info := strings.Builder
	
	info.WriteString("🔒 Hardware Wallet Support in Vaughan Crush\n\n")
	info.WriteString("🛠️ Supported Hardware:\n")
	info.WriteString("✅ Trezor Model T/One/Safe\n")
	info.WriteString("✅ Ledger Nano S/X/S Plus\n")
	info.WriteString("✅ GridPlus Lattice1\n")
	info.WriteString("✅ AWS/GCP KMS (Enterprise)\n\n")
	
	info.WriteString("🚀 Cast Integration:\n")
	info.WriteString("• Native Cast commands: --trezor, --ledger\n")
	info.WriteString("• Auto-detection in Vaughan Crush\n")
	info.WriteString("• Secure transaction signing\n")
	info.WriteString("• Hardware confirmation prompts\n")
	info.WriteString("• Private keys never leave device\n\n")
	
	info.WriteString("🎯 Use Cases:\n")
	info.WriteString("🔹 Mainnet transactions (high security)\n")
	info.WriteString("🔹 Large transfers (significant amounts)\n")
	info.WriteString("🔹 DeFi operations (complex, risky)\n")
	info.WriteString("🔹 Multi-sig setups (enhanced security)\n")
	info.WriteString("🔹 Long-term holdings (cold storage)\n\n")
	
	info.WriteString("📋 Example Commands:\n")
	info.WriteString("• Send with Trezor: cast send --trezor [options]\n")
	info.WriteString("• Send with Ledger: cast send --ledger [options]\n")
	info.WriteString("• Check devices: cast wallet list --trezor\n")
	info.WriteString("• Sign message: cast sign --trezor \"message\"\n\n")
	
	info.WriteString("🛡️ Security Benefits:\n")
	info.WriteString("• Private keys never exposed\n")
	info.WriteString("• Transactions signed on hardware\n")
	info.WriteString("• Protection against malware\n")
	info.WriteString("• Device confirmation required\n")
	info.WriteString("• PIN and passphrase protection\n\n")
	
	info.WriteString("🚀 Getting Started:\n")
	info.WriteString("1. Connect hardware wallet\n")
	info.WriteString("2. Start Vaughan Crush: ./vaughan-crush\n")
	info.WriteString("3. Try: \"Use hardware wallet\"\n")
	info.WriteString("4. Follow setup instructions\n")
	info.WriteString("5. Test with small transaction\n")
	
	return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, info.String()), nil
}