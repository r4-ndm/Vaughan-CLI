package blockchain

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// NetworkConfig represents a blockchain network configuration
type NetworkConfig struct {
	Name      string `json:"name"`
	ChainID   int    `json:"chain_id"`
	RPCUrl    string `json:"rpc_url"`
	BlockTime int    `json:"block_time"`
	GasToken  string `json:"gas_token"`
}

// WalletConfig represents a wallet/identity configuration
type WalletConfig struct {
	Name        string `json:"name"`
	Address     string `json:"address"`
	PrivateKey  string `json:"private_key,omitempty"`
	Mnemonic    string `json:"mnemonic,omitempty"`
	HDPath      string `json:"hd_path,omitempty"`
	Description string `json:"description,omitempty"`
}

// BlockchainConfig represents the blockchain configuration
type BlockchainConfig struct {
	DefaultNetwork string                   `json:"default_network"`
	Networks      map[string]NetworkConfig  `json:"networks"`
	Wallets       map[string]WalletConfig   `json:"wallets"`
	AddressBook   map[string]string         `json:"address_book"`
	GasStrategies map[string]GasStrategy    `json:"gas_strategies"`
}

// GasStrategy represents gas pricing strategies
type GasStrategy struct {
	Name         string `json:"name"`
	GasPrice     string `json:"gas_price,omitempty"`
	GasMultiplier float64 `json:"gas_multiplier"`
	Description  string `json:"description"`
}

// DefaultNetworks returns default network configurations
func DefaultNetworks() map[string]NetworkConfig {
	return map[string]NetworkConfig{
		"mainnet": {
			Name:      "Ethereum Mainnet",
			ChainID:   1,
			RPCUrl:    "https://eth.llamarpc.com",
			BlockTime: 12,
			GasToken:  "ETH",
		},
		"goerli": {
			Name:      "Goerli Testnet",
			ChainID:   5,
			RPCUrl:    "https://ethereum-goerli.publicnode.com",
			BlockTime: 15,
			GasToken:  "ETH",
		},
		"sepolia": {
			Name:      "Sepolia Testnet", 
			ChainID:   11155111,
			RPCUrl:    "https://ethereum-sepolia.publicnode.com",
			BlockTime: 15,
			GasToken:  "ETH",
		},
		"polygon": {
			Name:      "Polygon Mainnet",
			ChainID:   137,
			RPCUrl:    "https://polygon.llamarpc.com",
			BlockTime: 2,
			GasToken:  "MATIC",
		},
		"anvil": {
			Name:      "Anvil Local Node",
			ChainID:   31337,
			RPCUrl:    "http://127.0.0.1:8545",
			BlockTime: 0,
			GasToken:  "ETH",
		},
	}
}

// DefaultGasStrategies returns default gas strategies
func DefaultGasStrategies() map[string]GasStrategy {
	return map[string]GasStrategy{
		"slow": {
			Name:         "Slow",
			GasMultiplier: 0.9,
			Description:  "Low gas fees, longer wait time",
		},
		"standard": {
			Name:         "Standard", 
			GasMultiplier: 1.0,
			Description:  "Balanced gas fees and speed",
		},
		"fast": {
			Name:         "Fast",
			GasMultiplier: 1.2,
			Description:  "Higher gas fees, quick confirmation",
		},
		"urgent": {
			Name:         "Urgent",
			GasMultiplier: 1.5,
			Description:  "Maximum gas fees, fastest confirmation",
		},
	}
}

// LoadBlockchainConfig loads blockchain configuration from file
func LoadBlockchainConfig(configPath string) (*BlockchainConfig, error) {
	config := &BlockchainConfig{
		Networks:      DefaultNetworks(),
		GasStrategies: DefaultGasStrategies(),
		AddressBook:   make(map[string]string),
		Wallets:       make(map[string]WalletConfig),
		DefaultNetwork: "mainnet",
	}

	// Try to load existing config
	if data, err := os.ReadFile(configPath); err == nil {
		if err := json.Unmarshal(data, config); err != nil {
			return nil, fmt.Errorf("failed to parse blockchain config: %w", err)
		}
	}

	return config, nil
}

// SaveBlockchainConfig saves blockchain configuration to file
func (c *BlockchainConfig) Save(configPath string) error {
	// Create directory if it doesn't exist
	if err := os.MkdirAll(filepath.Dir(configPath), 0755); err != nil {
		return fmt.Errorf("failed to create config directory: %w", err)
	}

	data, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal config: %w", err)
	}

	if err := os.WriteFile(configPath, data, 0600); err != nil {
		return fmt.Errorf("failed to write config file: %w", err)
	}

	return nil
}

// GetNetwork returns network configuration by name
func (c *BlockchainConfig) GetNetwork(name string) (NetworkConfig, error) {
	network, exists := c.Networks[name]
	if !exists {
		return NetworkConfig{}, fmt.Errorf("network '%s' not found", name)
	}
	return network, nil
}

// AddNetwork adds a new network configuration
func (c *BlockchainConfig) AddNetwork(name string, config NetworkConfig) {
	if c.Networks == nil {
		c.Networks = make(map[string]NetworkConfig)
	}
	c.Networks[name] = config
}

// ResolveAddress resolves an address using the address book
func (c *BlockchainConfig) ResolveAddress(address string) string {
	// Check if it's in the address book
	if resolved, exists := c.AddressBook[address]; exists {
		return resolved
	}
	
	// Check ENS names
	if strings.HasSuffix(address, ".eth") {
		return address // Will be resolved by cast
	}
	
	return address
}

// AddAddress adds an address to the address book
func (c *BlockchainConfig) AddAddress(name, address string) {
	if c.AddressBook == nil {
		c.AddressBook = make(map[string]string)
	}
	c.AddressBook[name] = address
}