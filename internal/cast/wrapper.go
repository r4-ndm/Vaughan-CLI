package cast

import (
	"context"
	"fmt"
	"os/exec"
	"strings"
	"time"
)

// CastWrapper provides high-level interface to Cast commands
type CastWrapper struct {
	DefaultRPC string
	Timeout    time.Duration
}

// NewCastWrapper creates a new Cast wrapper
func NewCastWrapper(rpc string) *CastWrapper {
	return &CastWrapper{
		DefaultRPC: rpc,
		Timeout:    30 * time.Second,
	}
}

// Call executes a cast call command
func (c *CastWrapper) Call(ctx context.Context, contract, function string, args []string, rpcUrl string) (string, error) {
	cmdParts := []string{"cast", "call", contract, function}
	cmdParts = append(cmdParts, args...)
	
	if rpcUrl == "" {
		rpcUrl = c.DefaultRPC
	}
	if rpcUrl != "" {
		cmdParts = append(cmdParts, "--rpc-url", rpcUrl)
	}
	
	return c.executeCommand(ctx, cmdParts)
}

// Send executes a cast send command
func (c *CastWrapper) Send(ctx context.Context, to, value, function string, args []string, options map[string]string) (string, error) {
	cmdParts := []string{"cast", "send", to}
	
	if value != "" {
		cmdParts = append(cmdParts, "--value", value)
	}
	
	if function != "" {
		cmdParts = append(cmdParts, function)
		cmdParts = append(cmdParts, args...)
	}
	
	// Add RPC URL if provided
	if rpcUrl, exists := options["rpc_url"]; exists && rpcUrl != "" {
		cmdParts = append(cmdParts, "--rpc-url", rpcUrl)
	} else if c.DefaultRPC != "" {
		cmdParts = append(cmdParts, "--rpc-url", c.DefaultRPC)
	}
	
	// Add other options
	for key, val := range options {
		if key == "rpc_url" {
			continue
		}
		cmdParts = append(cmdParts, "--"+key, val)
	}
	
	return c.executeCommand(ctx, cmdParts)
}

// GasPrice gets current gas prices
func (c *CastWrapper) GasPrice(ctx context.Context, rpcUrl string) (string, error) {
	cmdParts := []string{"cast", "gas-price"}
	
	if rpcUrl == "" {
		rpcUrl = c.DefaultRPC
	}
	if rpcUrl != "" {
		cmdParts = append(cmdParts, "--rpc-url", rpcUrl)
	}
	
	return c.executeCommand(ctx, cmdParts)
}

// Balance gets ETH balance of an address
func (c *CastWrapper) Balance(ctx context.Context, address string, rpcUrl string) (string, error) {
	cmdParts := []string{"cast", "balance", address}
	
	if rpcUrl == "" {
		rpcUrl = c.DefaultRPC
	}
	if rpcUrl != "" {
		cmdParts = append(cmdParts, "--rpc-url", rpcUrl)
	}
	
	return c.executeCommand(ctx, cmdParts)
}

// Tx gets transaction details
func (c *CastWrapper) Tx(ctx context.Context, hash string, rpcUrl string) (string, error) {
	cmdParts := []string{"cast", "tx", hash}
	
	if rpcUrl == "" {
		rpcUrl = c.DefaultRPC
	}
	if rpcUrl != "" {
		cmdParts = append(cmdParts, "--rpc-url", rpcUrl)
	}
	
	return c.executeCommand(ctx, cmdParts)
}

// executeCommand executes a Cast command and returns output
func (c *CastWrapper) executeCommand(ctx context.Context, parts []string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, c.Timeout)
	defer cancel()
	
	cmd := exec.CommandContext(ctx, parts[0], parts[1:]...)
	output, err := cmd.CombinedOutput()
	
	if err != nil {
		return "", fmt.Errorf("cast command failed: %w\nOutput: %s", err, string(output))
	}
	
	return strings.TrimSpace(string(output)), nil
}

// ValidateAddress performs basic address validation
func ValidateAddress(address string) error {
	if len(address) == 0 {
		return fmt.Errorf("address cannot be empty")
	}
	
	if strings.HasPrefix(address, "0x") {
		if len(address) != 42 {
			return fmt.Errorf("invalid hex address length: expected 42, got %d", len(address))
		}
		return nil
	}
	
	// For ENS names and other address formats, let Cast handle validation
	return nil
}