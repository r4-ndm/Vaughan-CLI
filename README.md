# Vaughan-CLI

<p align="center">
    <a href="https://github.com/r4-ndm/Vaughan-CLI"><img width="450" alt="Vaughan CLI Logo" src="Vaughan-CLI-logo/Vaughan-big.png" /></a><br />
    <a href="https://github.com/r4-ndm/Vaughan-CLI/releases"><img src="https://img.shields.io/github/release/r4-ndm/Vaughan-CLI" alt="Latest Release"></a>
    <a href="https://github.com/r4-ndm/Vaughan-CLI/actions"><img src="https://github.com/r4-ndm/Vaughan-CLI/actions/workflows/build.yml/badge.svg" alt="Build Status"></a>
</p>

<p align="center">AI-powered blockchain and programming CLI with enhanced security and privacy.</p>
<p align="center">🦊⚡ Your secure coding companion for blockchain development.</p>

## Features

- **🔒 Privacy-First:** No analytics, no telemetry, no data collection
- **🤖 Multi-Model:** Choose from a wide range of LLMs or add your own via OpenAI- or Anthropic-compatible APIs
- **⛓️ Blockchain-Native:** Built-in blockchain tools and security features
- **🔄 Flexible:** Switch LLMs mid-session while preserving context
- **📂 Session-Based:** Maintain multiple work sessions and contexts per project
- **🔍 LSP-Enhanced:** Uses LSPs for additional context, just like you do
- **🛠️ Extensible:** Add capabilities via MCPs (http, stdio, and sse)
- **🌍 Cross-Platform:** Works on macOS, Linux, Windows (PowerShell and WSL), FreeBSD, OpenBSD, and NetBSD

## Installation

### Quick Install (Recommended)

#### Go Install
```bash
go install github.com/r4-ndm/Vaughan-CLI@latest
```

#### Download Binary
```bash
# Linux
wget https://github.com/r4-ndm/Vaughan-CLI/releases/latest/download/vaughan-cli-linux-amd64
chmod +x vaughan-cli-linux-amd64
sudo mv vaughan-cli-linux-amd64 /usr/local/bin/vaughan-cli

# macOS
wget https://github.com/r4-ndm/Vaughan-CLI/releases/latest/download/vaughan-cli-darwin-amd64
chmod +x vaughan-cli-darwin-amd64
sudo mv vaughan-cli-darwin-amd64 /usr/local/bin/vaughan-cli

# Windows
wget https://github.com/r4-ndm/Vaughan-CLI/releases/latest/download/vaughan-cli-windows-amd64.exe
```

### Build from Source

```bash
git clone https://github.com/r4-ndm/Vaughan-CLI.git
cd Vaughan-CLI
go build .
./vaughan-cli
```

## Getting Started

The quickest way to get started is to grab an API key for your preferred provider such as Anthropic, OpenAI, Groq, or OpenRouter and just start Vaughan-CLI. You'll be prompted to enter your API key.

### Environment Variables

You can also set environment variables for preferred providers:

| Environment Variable        | Provider                                           |
| --------------------------- | -------------------------------------------------- |
| `ANTHROPIC_API_KEY`         | Anthropic                                          |
| `OPENAI_API_KEY`            | OpenAI                                             |
| `GROQ_API_KEY`              | Groq                                               |
| `OPENROUTER_API_KEY`        | OpenRouter                                         |

### Usage

```bash
# Start Vaughan CLI
vaughan-cli

# Check version
vaughan-cli --version

# Show help
vaughan-cli --help
```

## Privacy & Security

- ✅ **No Analytics:** No user tracking or data collection
- ✅ **No Telemetry:** All telemetry features have been removed
- ✅ **Local Processing:** All operations are performed locally
- ✅ **Secure Key Management:** Built-in encryption for sensitive data

## Configuration

Vaughan-CLI uses a simple JSON configuration file. The config file is automatically created on first run at:

- **Linux/macOS:** `~/.config/vaughan-cli/config.json`
- **Windows:** `%APPDATA%\vaughan-cli\config.json`

### Example Configuration

```json
{
  "$schema": "https://vaughan-cli.example.com/schema.json",
  "models": {
    "large": {
      "id": "claude-3-5-sonnet-20241022",
      "provider": "anthropic"
    }
  },
  "providers": {
    "anthropic": {
      "id": "anthropic",
      "name": "Anthropic",
      "base_url": "https://api.anthropic.com",
      "type": "anthropic",
      "api_key": "your-anthropic-api-key"
    }
  }
}
```

## Development

### Building

```bash
go build .
```

### Testing

```bash
go test ./...
```

### Formatting

```bash
gofumpt -w .
```

### Linting

```bash
golangci-lint run
```

## Contributing

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Support

- 📖 [Documentation](https://github.com/r4-ndm/Vaughan-CLI/wiki)
- 🐛 [Bug Reports](https://github.com/r4-ndm/Vaughan-CLI/issues)
- 💡 [Feature Requests](https://github.com/r4-ndm/Vaughan-CLI/issues)

---

**Built with ❤️ and 🦊⚡**