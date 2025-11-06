# Contributing to Vaughan Crush 🦊⚡

Thank you for your interest in contributing to Vaughan Crush! This guide will help you get started.

## 🚀 Getting Started

### Prerequisites
- Go 1.21 or higher
- Foundry installed (`curl -L https://foundry.paradigm.xyz | bash`)
- Git

### Development Setup
```bash
# Fork and clone the repository
git clone https://github.com/yourusername/vaughan-crush.git
cd vaughan-crush

# Install dependencies
go mod download

# Build the project
go build -o vaughan-crush ./cmd/cli

# Run tests
go test ./...
```

## 📁 Project Structure

```
vaughan-crush/
├── cmd/cli/              # Main CLI entrypoint
├── internal/
│   ├── agent/            # AI agent logic and templates
│   ├── blockchain/       # Blockchain-specific functionality
│   ├── config/           # Configuration management
│   ├── tui/              # Terminal user interface
│   └── tools/            # CLI tools (bash, edit, view, etc.)
├── vaughan-cli/          # Core Crush fork with blockchain enhancements
├── Modelfile            # AI model configuration
├── training-data/        # AI training examples
└── docs/               # Documentation
```

## 🛠️ How to Contribute

### Reporting Bugs
1. Search existing issues first
2. Use the bug report template
3. Include steps to reproduce
4. Provide system information (OS, Go version, etc.)

### Feature Requests
1. Check roadmap and existing requests
2. Provide clear use case
3. Consider implementation complexity
4. Be open to discussion

### Code Contributions
1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Make your changes
4. Add tests if applicable
5. Ensure code passes all checks
6. Submit a pull request

## 🧪 Testing

### Running Tests
```bash
# Run all tests
go test ./...

# Run specific package tests
go test ./internal/agent

# Run with coverage
go test -cover ./...
```

### Test Guidelines
- Write tests for new functionality
- Use testify for assertions
- Mock external dependencies
- Maintain test coverage above 80%

## 📝 Code Style

### Formatting
- Use `gofumpt -w .` for formatting
- Run `golangci-lint run` before submitting
- Follow Go conventions

### Naming
- Use PascalCase for exported types
- Use camelCase for variables and functions
- Be descriptive with names
- Avoid abbreviations unless common

### Documentation
- Add godoc comments to exported functions
- Update README for user-facing changes
- Include examples in documentation

## 🔄 Development Workflow

### Before Starting
1. Check existing issues and pull requests
2. Discuss major changes in an issue
3. Create a branch from main
4. Keep branches focused and small

### During Development
1. Commit frequently with clear messages
2. Use conventional commits (`feat:`, `fix:`, `docs:`, etc.)
3. Run tests locally before pushing
4. Update documentation as needed

### Submitting Changes
1. Ensure all tests pass
2. Update CHANGELOG.md if applicable
3. Rebase onto main if needed
4. Submit pull request with clear description
5. Respond to review feedback promptly

## 🏗️ Architecture Guidelines

### Core Components
- **Agent**: AI logic and prompt management
- **Config**: Configuration and provider management
- **Tools**: CLI tool implementations
- **Blockchain**: Cast integration and smart contract interaction

### Design Principles
- Keep components loosely coupled
- Use interfaces for extensibility
- Prioritize security and performance
- Maintain backward compatibility when possible

## 🔒 Security Considerations

- Never commit private keys or sensitive data
- Validate all user inputs
- Use testnet for blockchain operations by default
- Follow security best practices for smart contract interactions

## 📚 Resources

### Documentation
- [Go Documentation](https://golang.org/doc/)
- [Foundry Documentation](https://book.getfoundry.sh/)
- [Ollama Documentation](https://github.com/ollama/ollama)

### Community
- [Discussions](https://github.com/yourusername/vaughan-crush/discussions)
- [Issues](https://github.com/yourusername/vaughan-crush/issues)

## 🤝 Code of Conduct

Be respectful, inclusive, and constructive. We're here to build something amazing together!

---

Thank you for contributing to Vaughan Crush! 🎉