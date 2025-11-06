package blockchain

import (
	"context"
	_ "embed"
	"fmt"
	"html/template"
	"strings"

	"github.com/r4v3n/vaughan-cli/internal/fantasy"
	"github.com/r4v3n/vaughan-cli/internal/config"
	"github.com/r4v3n/vaughan-cli/internal/permission"
	"github.com/r4v3n/vaughan-cli/internal/shell"
)

type CastCallParams struct {
	ContractAddress string `json:"contract_address" description:"The contract address to call"`
	Function        string `json:"function" description:"The function signature to call (e.g., 'balanceOf(address)')"`
	Arguments       string `json:"arguments,omitempty" description:"Arguments for the function call (space-separated)"`
	RPCUrl          string `json:"rpc_url,omitempty" description:"RPC URL for the network (defaults to current network)"`
}

type CastCallResponse struct {
	Command  string `json:"command"`
	Output   string `json:"output"`
	Network  string `json:"network"`
	Function string `json:"function"`
}

const (
	CastCallToolName = "cast_call"
)

//go:embed cast_call.tpl
var castCallTemplate string

func CastCallTool(_ context.Context, toolInput *fantasy.ToolInput) (*fantasy.ToolResult, error) {
	var params CastCallParams
	if err := toolInput.UnmarshalParameters(&params); err != nil {
		return nil, fmt.Errorf("failed to parse parameters: %w", err)
	}

	// Build cast command
	cmdParts := []string{"cast", "call", params.ContractAddress, params.Function}
	if params.Arguments != "" {
		cmdParts = append(cmdParts, strings.Fields(params.Arguments)...)
	}
	if params.RPCUrl != "" {
		cmdParts = append(cmdParts, "--rpc-url", params.RPCUrl)
	}

	cmd := strings.Join(cmdParts, " ")

	// Check permissions
	allowed := permission.IsToolAllowed(CastCallToolName, map[string]any{
		"command": cmd,
	})
	if !allowed {
		return nil, permission.ErrPermissionDenied
	}

	// Execute command
	output, err := shell.Exec(cmd, "")
	if err != nil {
		return fantasy.NewToolResult(
			fantasy.ToolResultTypeError,
			"Failed to execute cast call: "+err.Error(),
		), nil
	}

	response := CastCallResponse{
		Command:  cmd,
		Output:   strings.TrimSpace(output),
		Network:  getDefaultNetwork(params.RPCUrl),
		Function: params.Function,
	}

	result, err := formatCastCallResponse(response)
	if err != nil {
		return fantasy.NewToolResult(
			fantasy.ToolResultTypeError,
			"Failed to format response: "+err.Error(),
		), nil
	}

	return fantasy.NewToolResult(fantasy.ToolResultTypeText, result), nil
}

func getDefaultNetwork(rpcUrl string) string {
	if rpcUrl == "" {
		cfg := config.Get()
		if cfg != nil && cfg.Providers != nil {
			providers := make(map[string]config.ProviderConfig)
			for k, v := range cfg.Providers.Seq2() {
				providers[k] = v
			}
			if len(providers) > 0 {
				// For now, return a default - in future, read from config
				return "mainnet"
			}
		}
		return "mainnet"
	}
	
	// Extract network name from common RPC URLs
	if strings.Contains(rpcUrl, "mainnet") {
		return "mainnet"
	} else if strings.Contains(rpcUrl, "goerli") {
		return "goerli"
	} else if strings.Contains(rpcUrl, "sepolia") {
		return "sepolia"
	} else if strings.Contains(rpcUrl, "polygon") {
		return "polygon"
	}
	
	return "custom"
}

func formatCastCallResponse(response CastCallResponse) (string, error) {
	tmpl, err := template.New("cast_call").Parse(castCallTemplate)
	if err != nil {
		return "", err
	}

	var buf strings.Builder
	if err := tmpl.Execute(&buf, response); err != nil {
		return "", err
	}

	return buf.String(), nil
}