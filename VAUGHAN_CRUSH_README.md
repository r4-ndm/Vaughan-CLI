# Vaughan Crush 🔗

AI-powered Cast helper for blockchain interactions, based on the original Crush framework by Charmbracelet.

Vaughan Crush is a specialized blockchain fork of Crush that maintains the powerful AI coding assistant
capabilities while focusing on smart contract interactions and blockchain development workflows.

---

## 🎯 What Makes Vaughan Crush Special?

**Original Crush Foundation**: Built on the proven Crush framework with its:
- Multi-model LLM support (OpenAI, Anthropic, Groq, etc.)
- Session-based workflows with context preservation
- LSP-enhanced code understanding
- Extensible MCP (Model Context Protocol) support
- Beautiful terminal UI

**Blockchain Specialization**: Enhanced with:
- 🤖 Natural language blockchain interactions
- 🔗 Cast (Foundry) integration for professional development
- ⚡ Smart gas optimization and cost analysis
- 🛡️ Security-first transaction handling
- 🌐 Multi-network support (Ethereum, testnets, Layer 2s)

---

## Quick Start

```bash
# Clone repository
git clone https://github.com/r4v3n/vaughan-cli.git
cd vaughan-cli

# Build project
go build -o vaughan-crush .

# Run Vaughan Crush
./vaughan-crush
```

## Natural Language Blockchain Examples

```
User: "What's the balance of vitalik.eth?"
Vaughan Crush: I'll check vitalik.eth's balance...
[Uses cast call to check balance]
Balance: 1,234.56 ETH

User: "Send 0.1 ETH to my friend"
Vaughan Crush: I'll send 0.1 ETH. Estimated gas cost: ~$2.50. Confirm?
[After confirmation]
Transaction sent! Hash: 0x1234...

User: "Check my USDC allowance on Uniswap"
Vaughan Crush: I'll check your USDC allowance...
[Uses cast call on USDC contract]
Current allowance: 500.00 USDC
```

## Configuration

Vaughan Crush uses `vaughan-crush.json` for configuration:

```json
{
  "models": {
    "large": {
      "model": "gpt-4o",
      "provider": "openai"
    }
  },
  "providers": {
    "openai": {
      "type": "openai",
      "api_key": "$OPENAI_API_KEY"
    }
  },
  "blockchain": {
    "default_network": "mainnet",
    "networks": {
      "mainnet": {
        "name": "Ethereum Mainnet",
        "rpc_url": "https://eth.llamarpc.com"
      }
    }
  }
}
```

## Blockchain Features

### Security First
- ⚠️ **Transaction Confirmations**: Always confirms before spending funds
- 🔒 **Private Key Protection**: Never exposes keys in responses
- 🧪 **Testnet Recommendations**: Suggests testnet usage first
- 📊 **Gas Cost Estimates**: Shows costs before execution
- ✅ **Address Validation**: Validates addresses before use

### AI Tools Available
- **cast_call**: Read contract functions (balances, view functions)
- **cast_send**: Send transactions (transfers, state changes)
- **gas_price**: Check current gas prices
- **view**: Read contract files and ABIs
- **bash**: Execute other Cast commands

### Network Support
Built-in support for:
- Ethereum Mainnet
- Sepolia Testnet
- Goerli Testnet
- Polygon Mainnet
- Anvil (local development)
- Custom RPC endpoints

---

## Credits & Attribution

**Original Crush Framework**: This project is a specialized fork of [Crush](https://github.com/charmbracelet/crush) by [Charmbracelet](https://charm.sh/). Crush provides the foundational AI assistant capabilities, beautiful TUI, and LLM integration that makes Vaughan Crush possible.

**Original Crush Features We Inherit**:
- Multi-model LLM support and switching
- Session-based workflows with context
- LSP-enhanced code understanding
- MCP (Model Context Protocol) extensibility
- Beautiful terminal UI and user experience
- Cross-platform compatibility
- Advanced tool execution system

**Vaughan Crush Additions**:
- Blockchain-specific AI tools (Cast integration)
- Security-focused transaction handling
- Multi-network support
- Gas optimization and cost analysis
- Smart contract interaction workflows

---

## License

FSL-1.1-MIT License (same as original Crush)

---

Built with ❤️ using the **Crush framework** and enhanced for blockchain developers.