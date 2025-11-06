package config

import (
	"encoding/json"
	"fmt"
	"os"

	"github.com/r4v3n/vaughan-cli/internal/blockchain"
	"github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// Manager implements ConfigManager interface
type Manager struct {
	configPath string
	config     *Config
}

// Config represents Vaughan Crush configuration
type Config struct {
	$Schema    string                `json:"$schema"`
	Models     Models               `json:"models"`
	Providers  Providers            `json:"providers"`
	Agents     Agents               `json:"agents"`
	Blockchain BlockchainConfig     `json:"blockchain"`
	LSP        LSPConfig            `json:"lsp"`
}

// Models configuration
type Models struct {
	Large Model `json:"large"`
}

// Model configuration
type Model struct {
	Model    string `json:"model"`
	Provider string `json:"provider"`
}

// Providers configuration
type Providers struct {
	LocalOllama ProviderConfig `json:"local-ollama"`
}

// ProviderConfig configuration
type ProviderConfig struct {
	Type     string          `json:"type"`
	BaseURL  string          `json:"base_url"`
	Models   []ModelDetails   `json:"models"`
}

// ModelDetails configuration
type ModelDetails struct {
	ID                 string  `json:"id"`
	Name               string  `json:"name"`
	CostPer1MIn        float64 `json:"cost_per_1m_in"`
	CostPer1MOut       float64 `json:"cost_per_1m_out"`
	CostPer1MInCached  float64 `json:"cost_per_1m_in_cached"`
	CostPer1MOutCached float64 `json:"cost_per_1m_out_cached"`
	ContextWindow      int     `json:"context_window"`
	DefaultMaxTokens   int     `json:"default_max_tokens"`
}

// Agents configuration
type Agents struct {
	Blockchain Agent `json:"blockchain"`
}

// Agent configuration
type Agent struct {
	ID          string   `json:"id"`
	Name        string   `json:"name"`
	Description string   `json:"description"`
	Model       string   `json:"model"`
	AllowedTools []string `json:"allowed_tools"`
}

// BlockchainConfig configuration
type BlockchainConfig struct {
	DefaultNetwork string                     `json:"default_network"`
	Networks     map[string]blockchain.Network `json:"networks"`
	AddressBook  map[string]string              `json:"address_book"`
	GasStrategies map[string]GasStrategy       `json:"gas_strategies"`
}

// GasStrategy configuration
type GasStrategy struct {
	Name           string  `json:"name"`
	GasMultiplier  float64 `json:"gas_multiplier"`
	Description    string  `json:"description"`
}

// LSPConfig configuration
type LSPConfig struct {
	Gopls interface{} `json:"gopls"`
}

// NewManager creates a new configuration manager
func NewManager(configPath string) interfaces.ConfigManager {
	return &Manager{
		configPath: configPath,
		config:     &Config{},
	}
}

// Load implements ConfigManager interface
func (m *Manager) Load() error {
	data, err := os.ReadFile(m.configPath)
	if err != nil {
		if os.IsNotExist(err) {
			return m.createDefaultConfig()
		}
		return fmt.Errorf("failed to read config file: %w", err)
	}

	if err := json.Unmarshal(data, m.config); err != nil {
		return fmt.Errorf("failed to parse config file: %w", err)
	}

	return m.Validate()
}

// Save implements ConfigManager interface
func (m *Manager) Save() error {
	data, err := json.MarshalIndent(m.config, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal config: %w", err)
	}

	if err := os.WriteFile(m.configPath, data, 0644); err != nil {
		return fmt.Errorf("failed to write config file: %w", err)
	}

	return nil
}

// GetNetwork implements ConfigManager interface
func (m *Manager) GetNetwork(networkName string) (interfaces.BlockchainNetwork, error) {
	if m.config.Blockchain.Networks == nil {
		return nil, fmt.Errorf("no networks configured")
	}

	network, exists := m.config.Blockchain.Networks[networkName]
	if !exists {
		return nil, fmt.Errorf("network '%s' not found", networkName)
	}

	if !network.IsValid() {
		return nil, fmt.Errorf("network '%s' configuration is invalid", networkName)
	}

	return &network, nil
}

// GetNetworks implements ConfigManager interface
func (m *Manager) GetNetworks() []interfaces.BlockchainNetwork {
	var networks []interfaces.BlockchainNetwork
	for name, network := range m.config.Blockchain.Networks {
		if network.IsValid() {
			network.Name = name // Ensure name matches key
			networks = append(networks, &network)
		}
	}
	return networks
}

// GetDefaultNetwork implements ConfigManager interface
func (m *Manager) GetDefaultNetwork() string {
	return m.config.Blockchain.DefaultNetwork
}

// SetDefaultNetwork implements ConfigManager interface
func (m *Manager) SetDefaultNetwork(networkName string) error {
	if _, err := m.GetNetwork(networkName); err != nil {
		return fmt.Errorf("cannot set default network: %w", err)
	}

	m.config.Blockchain.DefaultNetwork = networkName
	return nil
}

// Validate implements ConfigManager interface
func (m *Manager) Validate() error {
	if m.config.Blockchain.DefaultNetwork == "" {
		return fmt.Errorf("default_network not specified")
	}

	if _, err := m.GetNetwork(m.config.Blockchain.DefaultNetwork); err != nil {
		return fmt.Errorf("default network validation failed: %w", err)
	}

	if m.config.Models.Large.Model == "" {
		return fmt.Errorf("large model not specified")
	}

	if m.config.Providers.LocalOllama.Type == "" {
		return fmt.Errorf("local-ollama provider type not specified")
	}

	return nil
}

// createDefaultConfig creates a default configuration file
func (m *Manager) createDefaultConfig() error {
	m.config = &Config{
		$Schema: "https://charm.land/vaughan.json",
		Models: Models{
			Large: Model{
				Model:    "vaughan-crush-v1",
				Provider: "local-ollama",
			},
		},
		Providers: Providers{
			LocalOllama: ProviderConfig{
				Type:    "openai-compat",
				BaseURL: "http://127.0.0.1:11434/v1/",
				Models: []ModelDetails{
					{
						ID:                "vaughan-crush-v1",
						Name:              "Vaughan Crush v1 (Blockchain Specialized)",
						CostPer1MIn:       0,
						CostPer1MOut:      0,
						CostPer1MInCached:  0,
						CostPer1MOutCached: 0,
						ContextWindow:      32768,
						DefaultMaxTokens:   2000,
					},
				},
			},
		},
		Agents: Agents{
			Blockchain: Agent{
				ID:          "blockchain",
				Name:        "Vaughan Crush v1 - Blockchain AI Assistant",
				Description: "AI assistant specialized in blockchain interactions and Cast commands (custom trained)",
				Model:       "large",
				AllowedTools: []string{"cast_call", "cast_send", "gas_price", "view", "ls", "grep", "bash"},
			},
		},
		Blockchain: BlockchainConfig{
			DefaultNetwork: "sepolia",
			Networks: map[string]blockchain.Network{
				"mainnet": {
					Name:        "Ethereum Mainnet",
					ChainID:     1,
					RPCUrl:      "https://eth.llamarpc.com",
					BlockTime:   12,
					GasToken:    "ETH",
					Explorer:    "https://etherscan.io",
					Type:        "mainnet",
				},
				"sepolia": {
					Name:        "Sepolia Testnet",
					ChainID:     11155111,
					RPCUrl:      "https://ethereum-sepolia.publicnode.com",
					BlockTime:   15,
					GasToken:    "ETH",
					Explorer:    "https://sepolia.etherscan.io",
					Type:        "testnet",
				},
			},
			AddressBook: map[string]string{
				"vitalik":      "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045",
				"uniswap_v3":   "0xE592427A0AEce92De3Edee1F18E0157C05861564",
			},
		},
	}

	return m.Save()
}

// GetModel returns the specified model configuration
func (m *Manager) GetModel(modelName string) (Model, error) {
	switch modelName {
	case "large":
		return m.config.Models.Large, nil
	default:
		return Model{}, fmt.Errorf("model '%s' not found", modelName)
	}
}

// GetProvider returns the specified provider configuration
func (m *Manager) GetProvider(providerName string) (ProviderConfig, error) {
	switch providerName {
	case "local-ollama":
		return m.config.Providers.LocalOllama, nil
	default:
		return ProviderConfig{}, fmt.Errorf("provider '%s' not found", providerName)
	}
}