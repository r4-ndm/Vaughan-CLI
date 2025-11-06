# Plugin Development Examples

This directory contains examples of creating plugins for Vaughan Crush.

## Table of Contents

- [Blockchain Network Plugin](#blockchain-network-plugin)
- [Configuration Plugin](#configuration-plugin)
- [Tool Plugin](#tool-plugin)
- [Model Provider Plugin](#model-provider-plugin)

---

## Blockchain Network Plugin

Example of creating a custom blockchain network plugin.

### Implementation

```go
package main

import (
    "fmt"
    "github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// ArbitrumNetwork implements BlockchainNetwork interface
type ArbitrumNetwork struct {
    Name        string
    ChainID     int
    RPCUrl      string
    BlockTime   int
    GasToken    string
    Explorer    string
    Type        string
}

func (n *ArbitrumNetwork) GetName() string {
    return n.Name
}

func (n *ArbitrumNetwork) GetChainID() int {
    return n.ChainID
}

func (n *ArbitrumNetwork) GetRPCURL() string {
    return n.RPCUrl
}

func (n *ArbitrumNetwork) GetGasToken() string {
    return n.GasToken
}

func (n *ArbitrumNetwork) GetBlockTime() int {
    return n.BlockTime
}

func (n *ArbitrumNetwork) GetExplorerURL() string {
    return n.Explorer
}

func (n *ArbitrumNetwork) GetType() string {
    return n.Type
}

func main() {
    // Create Arbitrum network
    arbitrum := &ArbitrumNetwork{
        Name:        "Arbitrum One",
        ChainID:     42161,
        RPCUrl:      "https://arb1.arbitrum.io/rpc",
        BlockTime:   12,
        GasToken:    "ETH",
        Explorer:    "https://arbiscan.io",
        Type:        "mainnet",
    }

    fmt.Printf("Arbitrum plugin loaded: %s (Chain ID: %d)\n", 
        arbitrum.GetName(), arbitrum.GetChainID())
}
```

### Building the Plugin

```bash
# Build as a Go module
go build -o arbitrum-plugin ./examples/arbitrum/

# Plugin would be dynamically loaded by Vaughan Crush
```

---

## Configuration Plugin

Example of creating a custom configuration provider.

### Implementation

```go
package main

import (
    "encoding/json"
    "fmt"
    "os"
    "github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// CustomConfigManager implements ConfigManager interface
type CustomConfigManager struct {
    filePath string
    networks map[string]interfaces.BlockchainNetwork
    defaultNetwork string
}

func NewCustomConfigManager(filePath string) *CustomConfigManager {
    return &CustomConfigManager{
        filePath: filePath,
        networks: make(map[string]interfaces.BlockchainNetwork),
    }
}

func (c *CustomConfigManager) Load() error {
    data, err := os.ReadFile(c.filePath)
    if err != nil {
        return fmt.Errorf("failed to read config: %w", err)
    }

    var config struct {
        DefaultNetwork string                      `json:"default_network"`
        Networks      map[string]json.RawMessage   `json:"networks"`
    }

    if err := json.Unmarshal(data, &config); err != nil {
        return fmt.Errorf("failed to parse config: %w", err)
    }

    c.defaultNetwork = config.DefaultNetwork

    // Parse network configurations
    for name, rawNetwork := range config.Networks {
        // Implement custom network parsing logic here
        fmt.Printf("Loading network: %s\n", name)
    }

    return nil
}

func (c *CustomConfigManager) Save() error {
    // Implement custom save logic
    return nil
}

func (c *CustomConfigManager) GetNetwork(networkName string) (interfaces.BlockchainNetwork, error) {
    network, exists := c.networks[networkName]
    if !exists {
        return nil, fmt.Errorf("network not found: %s", networkName)
    }
    return network, nil
}

func (c *CustomConfigManager) GetNetworks() []interfaces.BlockchainNetwork {
    var networks []interfaces.BlockchainNetwork
    for _, network := range c.networks {
        networks = append(networks, network)
    }
    return networks
}

func (c *CustomConfigManager) GetDefaultNetwork() string {
    return c.defaultNetwork
}

func (c *CustomConfigManager) SetDefaultNetwork(networkName string) error {
    if _, err := c.GetNetwork(networkName); err != nil {
        return err
    }
    c.defaultNetwork = networkName
    return nil
}

func (c *CustomConfigManager) Validate() error {
    // Implement custom validation logic
    if c.defaultNetwork == "" {
        return fmt.Errorf("default network not specified")
    }
    return nil
}

func main() {
    config := NewCustomConfigManager("custom-config.json")
    if err := config.Load(); err != nil {
        fmt.Printf("Error loading config: %v\n", err)
        return
    }

    fmt.Printf("Custom config plugin loaded with %d networks\n", 
        len(config.GetNetworks()))
}
```

---

## Tool Plugin

Example of creating a custom AI tool.

### Implementation

```go
package main

import (
    "context"
    "fmt"
    "encoding/json"
    "github.com/r4v3n/vaughan-cli/internal/fantasy"
)

// DeFiAnalyzer analyzes DeFi protocols
type DeFiAnalyzer struct{}

type DeFiInput struct {
    Protocol   string `json:"protocol"`
    Address    string `json:"address"`
    Action     string `json:"action"`
    Token      string `json:"token"`
    Amount     string `json:"amount"`
}

type DeFiResult struct {
    Protocol   string `json:"protocol"`
    Action     string `json:"action"`
    Analysis   string `json:"analysis"`
    Risk       string `json:"risk"`
    Recommendation string `json:"recommendation"`
    CastCommand string `json:"cast_command"`
}

func (t *DeFiAnalyzer) Execute(ctx context.Context, input *fantasy.ToolInput) (*fantasy.ToolResult, error) {
    var toolInput DeFiInput
    if err := input.UnmarshalParameters(&toolInput); err != nil {
        return fantasy.NewToolResult(fantasy.ToolResultTypeError, "Invalid input"), nil
    }

    // Perform DeFi analysis
    result := DeFiResult{
        Protocol: toolInput.Protocol,
        Action:   toolInput.Action,
        Analysis: fmt.Sprintf("Analyzing %s transaction on %s", 
            toolInput.Action, toolInput.Protocol),
        Risk:      "Medium - always verify contracts",
        Recommendation: "Use small test amounts first",
        CastCommand: fmt.Sprintf("cast call %s \"transfer(address,uint256)\" %s %s",
            toolInput.Address, toolInput.Token, toolInput.Amount),
    }

    resultJSON, _ := json.MarshalIndent(result, "", "  ")
    return fantasy.NewToolResult(fantasy.ToolResultTypeSuccess, string(resultJSON)), nil
}

func main() {
    // Create the DeFi analyzer tool
    tool := fantasy.NewAgentToolWithAllParams(
        "defi_analyzer",
        "DeFi Protocol Analyzer",
        "Analyzes DeFi protocol transactions for safety, risk, and provides Cast commands",
        "Use this tool when:\n- User wants to interact with DeFi protocols\n- Need risk assessment for transactions\n- Want Cast commands for DeFi operations\n- Analyzing protocol interactions",
        t.Execute,
    )

    fmt.Printf("DeFi Analyzer tool plugin loaded: %s\n", tool.GetName())
}
```

---

## Model Provider Plugin

Example of creating a custom AI model provider.

### Implementation

```go
package main

import (
    "context"
    "fmt"
    "bytes"
    "net/http"
    "encoding/json"
    "github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// CustomModelProvider connects to custom AI service
type CustomModelProvider struct {
    Name      string
    Type      string
    Model     string
    Endpoint  string
    APIKey    string
}

type ModelRequest struct {
    Model  string `json:"model"`
    Prompt string `json:"prompt"`
    MaxTokens int  `json:"max_tokens"`
}

type ModelResponse struct {
    Response string `json:"response"`
    Usage    struct {
        InputTokens  int `json:"input_tokens"`
        OutputTokens int `json:"output_tokens"`
    } `json:"usage"`
}

func (p *CustomModelProvider) GetName() string {
    return p.Name
}

func (p *CustomModelProvider) GetType() string {
    return p.Type
}

func (p *CustomModelProvider) GetModel() string {
    return p.Model
}

func (p *CustomModelProvider) GetEndpoint() string {
    return p.Endpoint
}

func (p *CustomModelProvider) IsAvailable() bool {
    return p.APIKey != ""
}

func (p *CustomModelProvider) Generate(ctx context.Context, prompt string) (string, error) {
    request := ModelRequest{
        Model:     p.Model,
        Prompt:    prompt,
        MaxTokens: 2000,
    }

    requestBody, err := json.Marshal(request)
    if err != nil {
        return "", err
    }

    req, err := http.NewRequestWithContext(ctx, "POST", p.Endpoint, bytes.NewBuffer(requestBody))
    if err != nil {
        return "", err
    }

    req.Header.Set("Content-Type", "application/json")
    req.Header.Set("Authorization", "Bearer "+p.APIKey)

    client := &http.Client{}
    resp, err := client.Do(req)
    if err != nil {
        return "", err
    }
    defer resp.Body.Close()

    if resp.StatusCode != http.StatusOK {
        return "", fmt.Errorf("API request failed: %s", resp.Status)
    }

    var response ModelResponse
    if err := json.NewDecoder(resp.Body).Decode(&response); err != nil {
        return "", err
    }

    return response.Response, nil
}

func main() {
    provider := &CustomModelProvider{
        Name:     "Custom AI Provider",
        Type:     "custom-api",
        Model:    "custom-model-v1",
        Endpoint: "https://api.custom-ai.com/v1/generate",
        APIKey:   "your-api-key-here",
    }

    if !provider.IsAvailable() {
        fmt.Printf("Provider not available: API key missing\n")
        return
    }

    response, err := provider.Generate(context.Background(), "Hello, custom AI!")
    if err != nil {
        fmt.Printf("Generation failed: %v\n", err)
        return
    }

    fmt.Printf("Custom AI Provider loaded. Test response: %s\n", response)
}
```

---

## Plugin Registration

All plugins can be registered with the Vaughan Crush system:

### Blockchain Network Registration

```go
// Register network plugin
pluginSystem.RegisterBlockchainPlugin(
    "arbitrum",           // Plugin name
    "1.0.0",            // Version
    "Arbitrum One support", // Description
    "Your Name",         // Author
    arbitrumNetwork,      // Network implementation
)
```

### Configuration Provider Registration

```go
// Register config plugin
pluginSystem.RegisterConfigPlugin(
    "custom-config",      // Plugin name
    "1.0.0",           // Version
    "Custom config provider", // Description
    "Your Name",        // Author
    customConfigManager,   // Config manager implementation
)
```

---

## Plugin Lifecycle

1. **Discovery**: Vaughan Crush scans plugin directories
2. **Validation**: Plugins are validated for interface compliance
3. **Registration**: Plugins register with the system
4. **Initialization**: Plugins are initialized with dependencies
5. **Runtime**: Plugins respond to system events
6. **Shutdown**: Plugins gracefully shut down

---

## Best Practices

### Interface Compliance
- Ensure all interface methods are properly implemented
- Handle errors consistently
- Provide meaningful method implementations

### Error Handling
- Use standardized error types from `internal/errors`
- Provide context with errors
- Follow error propagation patterns

### Testing
- Write comprehensive unit tests
- Use mock implementations from `internal/test`
- Test edge cases and error conditions

### Documentation
- Document plugin functionality clearly
- Provide usage examples
- Include configuration instructions

---

## Building and Deploying

### Build Plugin
```bash
# Initialize Go module
go mod init my-vaughan-plugin

# Build plugin
go build -o my-plugin.so -buildmode=plugin .

# Or build as binary for integration
go build -o my-plugin .
```

### Deploy Plugin
```bash
# Copy to plugins directory
cp my-plugin.so ~/.vaughan-crush/plugins/

# Or install system-wide
sudo cp my-plugin.so /usr/local/lib/vaughan-crush/plugins/
```

### Configuration
```json
{
  "plugins": {
    "enabled": true,
    "directories": [
      "~/.vaughan-crush/plugins",
      "/usr/local/lib/vaughan-crush/plugins"
    ]
  }
}
```

---

## Community Support

- **GitHub Issues**: Report bugs and request features
- **Discord Community**: Get help from other developers
- **Documentation**: Check API documentation for latest interfaces
- **Examples**: Browse community-contributed plugins

*Happy plugin development!* 🚀