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

type GasPriceParams struct {
	RPCUrl string `json:"rpc_url,omitempty" description:"RPC URL for network (defaults to current network)"`
}

type GasPriceResponse struct {
	Command     string `json:"command"`
	Network     string `json:"network"`
	GasPrice    string `json:"gas_price"`
	BaseFee     string `json:"base_fee"`
	PriorityFee string `json:"priority_fee"`
	Error       string `json:"error,omitempty"`
}

const (
	GasPriceToolName = "gas_price"
)

//go:embed gas_price.tpl
var gasPriceTemplate string

func GasPriceTool(_ context.Context, toolInput *fantasy.ToolInput) (*fantasy.ToolResult, error) {
	var params GasPriceParams
	if err := toolInput.UnmarshalParameters(&params); err != nil {
		return nil, fmt.Errorf("failed to parse parameters: %w", err)
	}

	// Build cast command
	cmdParts := []string{"cast", "gas-price"}
	if params.RPCUrl != "" {
		cmdParts = append(cmdParts, "--rpc-url", params.RPCUrl)
	}

	cmd := strings.Join(cmdParts, " ")

	// Check permissions
	allowed := permission.IsToolAllowed(GasPriceToolName, map[string]any{
		"command": cmd,
	})
	if !allowed {
		return nil, permission.ErrPermissionDenied
	}

	// Execute command
	output, err := shell.Exec(cmd, "")
	if err != nil {
		response := GasPriceResponse{
			Command: cmd,
			Network: getDefaultNetwork(params.RPCUrl),
			Error:   err.Error(),
		}
		
		result, formatErr := formatGasPriceResponse(response)
		if formatErr != nil {
			return fantasy.NewToolResult(
				fantasy.ToolResultTypeError,
				"Failed to format error response: "+formatErr.Error(),
			), nil
		}
		
		return fantasy.NewToolResult(fantasy.ToolResultTypeText, result), nil
	}

	// Parse gas price output
	gasPrice, baseFee, priorityFee := parseGasPriceOutput(output)
	
	response := GasPriceResponse{
		Command:     cmd,
		Network:     getDefaultNetwork(params.RPCUrl),
		GasPrice:    gasPrice,
		BaseFee:     baseFee,
		PriorityFee: priorityFee,
	}

	result, err := formatGasPriceResponse(response)
	if err != nil {
		return fantasy.NewToolResult(
			fantasy.ToolResultTypeError,
			"Failed to format response: "+err.Error(),
		), nil
	}

	return fantasy.NewToolResult(fantasy.ToolResultTypeText, result), nil
}

func parseGasPriceOutput(output string) (gasPrice, baseFee, priorityFee string) {
	lines := strings.Split(strings.TrimSpace(output), "\n")
	
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if strings.Contains(line, "Gas price:") {
			parts := strings.Split(line, ":")
			if len(parts) > 1 {
				gasPrice = strings.TrimSpace(parts[1])
			}
		} else if strings.Contains(line, "Base fee:") {
			parts := strings.Split(line, ":")
			if len(parts) > 1 {
				baseFee = strings.TrimSpace(parts[1])
			}
		} else if strings.Contains(line, "Priority fee:") {
			parts := strings.Split(line, ":")
			if len(parts) > 1 {
				priorityFee = strings.TrimSpace(parts[1])
			}
		}
	}
	
	// If structured parsing failed, try to extract the first value as gas price
	if gasPrice == "" && len(lines) > 0 {
		gasPrice = strings.TrimSpace(lines[0])
	}
	
	return gasPrice, baseFee, priorityFee
}

func formatGasPriceResponse(response GasPriceResponse) (string, error) {
	tmpl, err := template.New("gas_price").Parse(gasPriceTemplate)
	if err != nil {
		return "", err
	}

	var buf strings.Builder
	if err := tmpl.Execute(&buf, response); err != nil {
		return "", err
	}

	return buf.String(), nil
}