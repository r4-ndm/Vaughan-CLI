# Vaughan Crush API Documentation

## Table of Contents

- [Core Interfaces](#core-interfaces)
- [Plugin Development](#plugin-development)
- [Examples](#examples)
- [Architecture](#architecture)

---

## Core Interfaces

### BlockchainNetwork Interface

Manages blockchain network configurations and operations.

```go
type BlockchainNetwork interface {
    GetName() string
    GetChainID() int
    GetRPCURL() string
    GetGasToken() string
    GetBlockTime() int
    GetExplorerURL() string
    GetType() string
}
```

#### Methods

| Method | Returns | Description |
|---------|----------|-------------|
| `GetName()` | string | Returns network name |
| `GetChainID()` | int | Returns blockchain chain ID |
| `GetRPCURL()` | string | Returns RPC endpoint URL |
| `GetGasToken()` | string | Returns gas token symbol |
| `GetBlockTime()` | int | Returns block time in seconds |
| `GetExplorerURL()` | string | Returns block explorer URL |
| `GetType()` | string | Returns network type (mainnet/testnet/local) |

#### Example Implementation

```go
type PulseChainNetwork struct {
    Name        string
    ChainID     int
    RPCUrl      string
    BlockTime   int
    GasToken    string
    Explorer    string
    Type        string
}

func (n *PulseChainNetwork) GetName() string {
    return n.Name
}

func (n *PulseChainNetwork) GetChainID() int {
    return n.ChainID
}

// ... implement other methods
```

---

### ConfigManager Interface

Handles configuration loading, validation, and management.

```go
type ConfigManager interface {
    Load() error
    Save() error
    GetNetwork(networkName string) (BlockchainNetwork, error)
    GetNetworks() []BlockchainNetwork
    GetDefaultNetwork() string
    SetDefaultNetwork(networkName string) error
    Validate() error
}
```

#### Methods

| Method | Returns | Description |
|---------|----------|-------------|
| `Load()` | error | Loads configuration from file |
| `Save()` | error | Saves configuration to file |
| `GetNetwork(networkName string)` | `(BlockchainNetwork, error)` | Returns specific network |
| `GetNetworks()` | `[]BlockchainNetwork` | Returns all networks |
| `GetDefaultNetwork()` | string | Returns default network name |
| `SetDefaultNetwork(networkName string)` | error | Sets default network |
| `Validate()` | error | Validates configuration |

---

### ToolProvider Interface

Manages AI tool registration and execution.

```go
type ToolProvider interface {
    GetName() string
    GetDescription() string
    GetTools() []AgentTool
    RegisterTool(tool AgentTool) error
    UnregisterTool(toolName string) error
}
```

#### Methods

| Method | Returns | Description |
|---------|----------|-------------|
| `GetName()` | string | Returns provider name |
| `GetDescription()` | string | Returns provider description |
| `GetTools()` | `[]AgentTool` | Returns all registered tools |
| `RegisterTool(tool AgentTool)` | error | Registers a new tool |
| `UnregisterTool(toolName string)` | error | Unregisters a tool |

---

### ErrorHandler Interface

Provides standardized error handling throughout the system.

```go
type ErrorHandler interface {
    HandleError(err error, context string) error
    LogError(err error, context string)
    GetUserMessage(err error) string
    ShouldRetry(err error) bool
}
```

#### Methods

| Method | Returns | Description |
|---------|----------|-------------|
| `HandleError(err error, context string)` | error | Handles error with context |
| `LogError(err error, context string)` | - | Logs error information |
| `GetUserMessage(err error)` | string | Returns user-friendly message |
| `ShouldRetry(err error)` | bool | Determines if operation should retry |

---

### UpdateManager Interface

Manages system updates from Crush framework.

```go
type UpdateManager interface {
    CheckForUpdates() (UpdateInfo, error)
    ApplyUpdate(updateInfo UpdateInfo) error
    GetCurrentVersion() string
    IsUpdateAvailable() bool
}
```

#### Methods

| Method | Returns | Description |
|---------|----------|-------------|
| `CheckForUpdates()` | `(UpdateInfo, error)` | Checks for available updates |
| `ApplyUpdate(updateInfo UpdateInfo)` | error | Applies an update |
| `GetCurrentVersion()` | string | Returns current version |
| `IsUpdateAvailable()` | bool | Checks if update is available |

#### UpdateInfo Structure

```go
type UpdateInfo struct {
    Version      string
    ReleaseNotes string
    DownloadURL  string
    Critical     bool
}
```

---

## Plugin Development

Vaughan Crush supports a robust plugin system for extending functionality.

### Plugin Types

1. **Blockchain Network Plugins**: Add new blockchain support
2. **Configuration Plugins**: Custom configuration providers
3. **Tool Plugins**: Extend AI capabilities
4. **Model Provider Plugins**: Add AI model support

### Creating a Blockchain Network Plugin

```go
package main

import (
    "github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// Custom blockchain network implementation
type CustomNetwork struct {
    Name        string
    ChainID     int
    RPCUrl      string
    GasToken    string
    BlockTime   int
    Explorer    string
    Type        string
}

// Implement BlockchainNetwork interface
func (n *CustomNetwork) GetName() string { return n.Name }
func (n *CustomNetwork) GetChainID() int { return n.ChainID }
func (n *CustomNetwork) GetRPCURL() string { return n.RPCUrl }
func (n *CustomNetwork) GetGasToken() string { return n.GasToken }
func (n *CustomNetwork) GetBlockTime() int { return n.BlockTime }
func (n *CustomNetwork) GetExplorerURL() string { return n.Explorer }
func (n *CustomNetwork) GetType() string { return n.Type }

// Register plugin
func main() {
    network := &CustomNetwork{
        Name:        "My Custom Network",
        ChainID:     999,
        RPCUrl:      "https://custom.rpc.url",
        GasToken:    "CUSTOM",
        BlockTime:   3,
        Explorer:    "https://custom.explorer.url",
        Type:        "mainnet",
    }

    // Register with plugin system
    pluginSystem.RegisterBlockchainPlugin(
        "custom-network",
        "1.0.0",
        "Custom blockchain network plugin",
        "Your Name",
        network,
    )
}
```

### Creating a Tool Plugin

```go
package main

import (
    "context"
    "github.com/r4v3n/vaughan-cli/internal/fantasy"
)

// Custom AI tool
type CustomTool struct{}

func (t *CustomTool) Execute(ctx context.Context, input *fantasy.ToolInput) (*fantasy.ToolResult, error) {
    return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, "Custom tool executed"), nil
}

// Register tool
func main() {
    tool := fantasy.NewAgentTool(
        "custom_tool",
        "Custom Tool",
        "Description of custom tool functionality",
        func(ctx context.Context, input *fantasy.ToolInput) (*fantasy.ToolResult, error) {
            return t.Execute(ctx, input)
        },
    )

    toolProvider.RegisterTool(tool)
}
```

---

## Dependency Injection

Vaughan Crush uses a dependency injection container for service management.

### Container Usage

```go
// Create container
container := container.NewContainer()

// Register services
configManager := config.NewManager("config.json")
container.RegisterConfigManager("default", configManager)

// Register blockchain network
pulsechain := &blockchain.Network{
    Name:        "PulseChain",
    ChainID:     369,
    RPCUrl:      "https://rpc.pulsechain.com",
    Type:        "mainnet",
}
container.RegisterBlockchainNetwork("pulsechain", pulsechain)

// Resolve services
var configManager interfaces.ConfigManager
err := container.ResolveInterface(&configManager)
if err != nil {
    log.Fatal(err)
}

var pulsechainNetwork interfaces.BlockchainNetwork
pulsechainNetwork, err := container.ResolveBlockchainNetwork("pulsechain")
if err != nil {
    log.Fatal(err)
}
```

---

## Event System

Vaughan Crush provides a publish/subscribe event system for loose coupling.

### Event Types

| Event Type | Description | Data |
|------------|-------------|-------|
| `error` | Error occurred | Error object |
| `network_change` | Network configuration changed | Old/new networks |
| `config_change` | Configuration changed | Changed fields |
| `transaction` | Transaction event | Transaction details |
| `plugin` | Plugin lifecycle event | Plugin info |

### Event System Usage

```go
// Create event system
eventSystem := events.NewSystem()

// Subscribe to events
eventSystem.Subscribe("network_change", func(event events.Event) error {
    oldNetwork := event.Data.(map[string]interface{})["old_network"]
    newNetwork := event.Data.(map[string]interface{})["new_network"]
    fmt.Printf("Network changed from %v to %v", oldNetwork, newNetwork)
    return nil
})

// Publish events
eventSystem.PublishNetworkChange(oldNetwork, newNetwork, "system")
eventSystem.PublishError(errors.New("test error"), "test_source")
```

---

## Examples

### Adding a New Blockchain

```go
// Create network implementation
arbitrum := &blockchain.Network{
    Name:        "Arbitrum One",
    ChainID:     42161,
    RPCUrl:      "https://arb1.arbitrum.io/rpc",
    BlockTime:   12,
    GasToken:    "ETH",
    Explorer:    "https://arbiscan.io",
    Type:        "mainnet",
}

// Register with configuration
configManager.SetNetwork("arbitrum", arbitrum)

// Update default network if needed
configManager.SetDefaultNetwork("arbitrum")
```

### Custom Error Handling

```go
// Create custom error
err := errors.ErrNetworkNotFound.WithContext("network", "unknown_network")

// Add user-friendly message
userMsg := errors.GetUserMessage(err)

// Check if retryable
if errors.IsRetryable(err) {
    // Retry logic
}
```

### Testing with Mocks

```go
func TestNetworkManager(t *testing.T) {
    // Create mocks
    mockNetwork := test.NewMockBlockchainNetwork()
    mockConfig := test.NewMockConfigManager()
    
    // Setup mock data
    mockConfig.NetworksValue = map[string]interfaces.BlockchainNetwork{
        "test": mockNetwork,
    }
    
    // Test
    network, err := mockConfig.GetNetwork("test")
    test.AssertNoError(t, err)
    test.AssertEqual(t, mockNetwork.GetName(), network.GetName())
}
```

---

## Architecture

### Modular Structure

```
internal/
├── interfaces/     📋 Core contracts
├── blockchain/     🌐 Network management
├── config/         ⚙️ Configuration system
├── plugin/         🔌 Plugin system
├── container/      📦 Dependency injection
├── events/         🔄 Event system
├── errors/         ❌ Error handling
├── update/         🔄 Update management
├── test/           🧪 Test infrastructure
└── agent/          🤖 AI orchestration
```

### Design Principles

1. **Interface Segregation**: Small, focused interfaces
2. **Dependency Inversion**: Depend on abstractions, not implementations
3. **Single Responsibility**: Each module has one purpose
4. **Open/Closed**: Open for extension, closed for modification
5. **Plugin Architecture**: Extensible through plugins

### Benefits

- 🏗️ **Maintainable**: Changes isolated to specific modules
- 🧪 **Testable**: Mockable interfaces enable comprehensive testing
- 🔄 **Extensible**: Plugin system allows easy feature addition
- 🛡️ **Reliable**: Standardized error handling and validation
- 🚀 **Performant**: Efficient dependency injection and event processing

---

## Contributing

### Development Guidelines

1. **Follow Interface Contracts**: Ensure implementations satisfy interfaces
2. **Write Tests**: Include unit tests for all components
3. **Error Handling**: Use standardized error types and patterns
4. **Documentation**: Maintain API documentation for all public APIs
5. **Plugin Development**: Use plugin system for extensions

### Code Style

- Use proper Go naming conventions
- Include godoc comments for all public functions
- Handle errors properly with context
- Write meaningful test cases
- Maintain backward compatibility

---

*This documentation covers the core API surface for Vaughan Crush development and extension.*