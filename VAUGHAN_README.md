# Vaughan Crush 🔗

AI-powered Cast helper for blockchain interactions, based on the original Crush framework by Charmbracelet.

Vaughan Crush is a specialized blockchain fork of Crush that maintains the powerful AI coding assistant
capabilities while focusing on smart contract interactions and blockchain development workflows.

## Features

**From Original Crush Framework:**
- **Multi-Model:** choose from a wide range of LLMs or add your own via OpenAI- or Anthropic-compatible APIs
- **Flexible:** switch LLMs mid-session while preserving context
- **Session-Based:** maintain multiple work sessions and contexts per project
- **LSP-Enhanced:** Crush uses LSPs for additional context, just like you do
- **Extensible:** add capabilities via MCPs (`http`, `stdio`, and `sse`)
- **Works Everywhere:** first-class support in every terminal on macOS, Linux, Windows (PowerShell and WSL), FreeBSD, OpenBSD, and NetBSD

**Blockchain Specialization:**
- 🤖 **Natural Language Interface**: Talk to blockchain in plain English
- 🔗 **Multi-Network Support**: Ethereum, Polygon, testnets, and custom RPCs
- ⚡ **Smart Gas Optimization**: AI suggests optimal gas strategies
- 🛡️ **Security First**: Built-in warnings and confirmations for transactions
- 📋 **Address Book**: Save and use named addresses
- 🔧 **Contract Interaction**: Call functions, send transactions, check balances

## Quick Start

### Installation

```bash
# Clone the repository
git clone https://github.com/r4v3n/vaughan-cli.git
cd vaughan-cli

# Build the project
go build -o vaughan-crush .

# Run Vaughan Crush
./vaughan-crush
```

### First Use

1. Run `./vaughan-crush` to start the interactive interface
2. Set up your AI provider (OpenAI, Anthropic, etc.)
3. Configure your blockchain networks and RPCs
4. Start interacting with contracts using natural language!

## Examples

### Check Balances
```
User: What's the balance of vitalik.eth?
Vaughan Crush: I'll check vitalik.eth's balance...
[Uses cast call to check balance]
Balance: 1,234.56 ETH
```

### Send Transactions
```
User: Send 0.1 ETH to my friend
Vaughan Crush: I'll send 0.1 ETH. Estimated gas cost: ~$2.50. Confirm?
[After confirmation]
Transaction sent! Hash: 0x1234...
```

### Contract Interactions
```
User: Check my USDC allowance on Uniswap
Vaughan Crush: I'll check your USDC allowance...
[Uses cast call on USDC contract]
Current allowance: 500.00 USDC
```

### Gas Optimization
```
User: What's the best gas price right now?
Vaughan Crush: [Checks current gas prices]
Current gas: 25 gwei
Recommended: Use "standard" strategy for best balance of cost/speed
```

## Configuration

Vaughan Crush uses `vaughan-crush.json` for configuration. Create one in your project root:

```json
{
  "agents": {
    "blockchain": {
      "id": "blockchain",
      "name": "Blockchain AI Assistant",
      "model": "large",
      "allowed_tools": ["cast_call", "cast_send", "gas_price"]
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

## Network Support

Built-in support for:
- Ethereum Mainnet
- Sepolia Testnet
- Goerli Testnet
- Polygon Mainnet
- Anvil (local development)
- Custom RPC endpoints

## Security Features

- ⚠️ **Transaction Confirmations**: Always confirms before spending funds
- 🔒 **Private Key Protection**: Never exposes keys in responses
- 🧪 **Testnet Recommendations**: Suggests testnet usage first
- 📊 **Gas Cost Estimates**: Shows costs before execution
- ✅ **Address Validation**: Validates addresses before use

## AI Tools Available

- **cast_call**: Read contract functions (balances, view functions)
- **cast_send**: Send transactions (transfers, state changes)
- **gas_price**: Check current gas prices
- **view**: Read contract files and ABIs
- **bash**: Execute other Cast commands

## Contributing

1. Fork the repository
2. Create a feature branch
3. Add your improvements
4. Open a pull request

## License

MIT License - see LICENSE file for details

## Roadmap

- [ ] DeFi protocol integrations
- [ ] Multi-wallet support
- [ ] Transaction scheduling
- [ ] Advanced contract analysis
- [ ] Portfolio tracking
- [ ] Web UI option

---

Built with ❤️ using the **Crush framework** and enhanced for blockchain development.

---

## Credits & Attribution

**Original Crush Framework**: This project is a specialized fork of [Crush](https://github.com/charmbracelet/crush) by [Charmbracelet](https://charm.sh/). Crush provides the foundational AI assistant capabilities, beautiful TUI, and LLM integration that make Vaughan Crush possible.

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