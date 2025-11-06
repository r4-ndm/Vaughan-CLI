package blockchain

import (
	"context"
	_ "embed"
	"fmt"
	"html/template"
	"strings"

	"github.com/r4v3n/vaughan-cli/internal/fantasy"
	"github.com/r4v3n/vaughan-cli/internal/permission"
	"github.com/r4v3n/vaughan-cli/internal/shell"
)

type CastSendParams struct {
	To           string `json:"to" description:"Recipient address or contract"`
	Value        string `json:"value,omitempty" description:"ETH value to send (in wei or with units like 1ether)"`
	Function     string `json:"function,omitempty" description:"Function signature to call (e.g., 'transfer(address,uint256)')"`
	Arguments    string `json:"arguments,omitempty" description:"Arguments for function call (space-separated)"`
	RPCUrl       string `json:"rpc_url,omitempty" description:"RPC URL for network"`
	GasPrice     string `json:"gas_price,omitempty" description:"Gas price (in wei)"`
	GasLimit     string `json:"gas_limit,omitempty" description:"Gas limit"`
	PrivateKey   string `json:"private_key,omitempty" description:"Private key (WARNING: use environment variables in production)"`
	Wallet       string `json:"wallet,omitempty" description:"Wallet name/alias from configuration"`
}

type CastSendResponse struct {
	Command     string `json:"command"`
	TxHash     string `json:"tx_hash,omitempty"`
	Network     string `json:"network"`
	To          string `json:"to"`
	Value       string `json:"value"`
	Error       string `json:"error,omitempty"`
	GasUsed     string `json:"gas_used,omitempty"`
}

const (
	CastSendToolName = "cast_send"
)

//go:embed cast_send.tpl
var castSendTemplate string

func CastSendTool(_ context.Context, toolInput *fantasy.ToolInput) (*fantasy.ToolResult, error) {
	var params CastSendParams
	if err := toolInput.UnmarshalParameters(&params); err != nil {
		return nil, fmt.Errorf("failed to parse parameters: %w", err)
	}

	// Validate required parameters
	if params.To == "" {
		return fantasy.NewToolResult(
			fantasy.ToolResultTypeError,
			"Missing required parameter: 'to'",
		), nil
	}

	// Build cast command
	cmdParts := []string{"cast", "send", params.To}
	
	if params.Value != "" {
		cmdParts = append(cmdParts, "--value", params.Value)
	}
	
	if params.Function != "" {
		cmdParts = append(cmdParts, params.Function)
		if params.Arguments != "" {
			cmdParts = append(cmdParts, strings.Fields(params.Arguments)...)
		}
	}
	
	if params.RPCUrl != "" {
		cmdParts = append(cmdParts, "--rpc-url", params.RPCUrl)
	}
	
	if params.GasPrice != "" {
		cmdParts = append(cmdParts, "--gas-price", params.GasPrice)
	}
	
	if params.GasLimit != "" {
		cmdParts = append(cmdParts, "--gas-limit", params.GasLimit)
	}
	
	if params.PrivateKey != "" {
		cmdParts = append(cmdParts, "--private-key", params.PrivateKey)
	} else if params.Wallet != "" {
		cmdParts = append(cmdParts, "--from", params.Wallet)
	}

	cmd := strings.Join(cmdParts, " ")

	// Check permissions
	allowed := permission.IsToolAllowed(CastSendToolName, map[string]any{
		"command": cmd,
		"to":      params.To,
		"value":   params.Value,
	})
	if !allowed {
		return nil, permission.ErrPermissionDenied
	}

	// Execute command
	output, err := shell.Exec(cmd, "")
	if err != nil {
		response := CastSendResponse{
			Command: cmd,
			Network: getDefaultNetwork(params.RPCUrl),
			To:      params.To,
			Value:   params.Value,
			Error:   err.Error(),
		}
		
		result, formatErr := formatCastSendResponse(response)
		if formatErr != nil {
			return fantasy.NewToolResult(
				fantasy.ToolResultTypeError,
				"Failed to format error response: "+formatErr.Error(),
			), nil
		}
		
		return fantasy.NewToolResult(fantasy.ToolResultTypeText, result), nil
	}

	// Parse transaction hash from output
	txHash := extractTxHash(output)
	
	response := CastSendResponse{
		Command: cmd,
		TxHash: txHash,
		Network: getDefaultNetwork(params.RPCUrl),
		To:      params.To,
		Value:   params.Value,
		GasUsed: extractGasUsed(output),
	}

	result, err := formatCastSendResponse(response)
	if err != nil {
		return fantasy.NewToolResult(
			fantasy.ToolResultTypeError,
			"Failed to format response: "+err.Error(),
		), nil
	}

	return fantasy.NewToolResult(fantasy.ToolResultTypeText, result), nil
}

func extractTxHash(output string) string {
	lines := strings.Split(output, "\n")
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "0x") && len(line) == 66 {
			return line
		}
		// Look for transaction hash in various formats
		if strings.Contains(line, "transaction hash") || strings.Contains(line, "tx") {
			parts := strings.Fields(line)
			for _, part := range parts {
				if strings.HasPrefix(part, "0x") && len(part) == 66 {
					return part
				}
			}
		}
	}
	return "Unknown"
}

func extractGasUsed(output string) string {
	lines := strings.Split(output, "\n")
	for _, line := range lines {
		if strings.Contains(strings.ToLower(line), "gas") {
			if strings.Contains(line, "used") || strings.Contains(line, ":") {
				parts := strings.Fields(line)
				for i, part := range parts {
					if strings.ToLower(part) == "gas" && i+1 < len(parts) {
						return parts[i+1]
					}
					if strings.Contains(part, ":") {
						gasPart := strings.Split(part, ":")
						if len(gasPart) == 2 {
							return strings.TrimSpace(gasPart[1])
						}
					}
				}
			}
		}
	}
	return "Unknown"
}

func formatCastSendResponse(response CastSendResponse) (string, error) {
	tmpl, err := template.New("cast_send").Parse(castSendTemplate)
	if err != nil {
		return "", err
	}

	var buf strings.Builder
	if err := tmpl.Execute(&buf, response); err != nil {
		return "", err
	}

	return buf.String(), nil
}