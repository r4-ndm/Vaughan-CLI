package blockchain

import (
	"github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// Network implements the BlockchainNetwork interface
type Network struct {
	Name        string `json:"name"`
	ChainID     int    `json:"chain_id"`
	RPCUrl      string `json:"rpc_url"`
	BlockTime   int    `json:"block_time"`
	GasToken    string `json:"gas_token"`
	Explorer    string `json:"explorer"`
	Faucet      string `json:"faucet,omitempty"`
	Type        string `json:"type"` // "mainnet", "testnet", "local"
}

// GetName returns the network name
func (n *Network) GetName() string {
	return n.Name
}

// GetChainID returns the network chain ID
func (n *Network) GetChainID() int {
	return n.ChainID
}

// GetRPCURL returns the network RPC URL
func (n *Network) GetRPCURL() string {
	return n.RPCUrl
}

// GetGasToken returns the network gas token
func (n *Network) GetGasToken() string {
	return n.GasToken
}

// GetBlockTime returns the network block time in seconds
func (n *Network) GetBlockTime() int {
	return n.BlockTime
}

// GetExplorerURL returns the network explorer URL
func (n *Network) GetExplorerURL() string {
	return n.Explorer
}

// GetType returns the network type (mainnet/testnet/local)
func (n *Network) GetType() string {
	return n.Type
}

// GetFaucet returns the network faucet URL (if available)
func (n *Network) GetFaucet() string {
	return n.Faucet
}

// IsValid checks if the network configuration is valid
func (n *Network) IsValid() bool {
	return n.Name != "" &&
		n.ChainID > 0 &&
		n.RPCUrl != "" &&
		n.GasToken != "" &&
		(n.Type == "mainnet" || n.Type == "testnet" || n.Type == "local")
}

// ToInterfaces converts Network slice to BlockchainNetwork interfaces
func ToInterfaces(networks []Network) []interfaces.BlockchainNetwork {
	result := make([]interfaces.BlockchainNetwork, len(networks))
	for i, n := range networks {
		result[i] = &n
	}
	return result
}