package blockchain

import (
	"context"
	"github.com/r4v3n/vaughan-cli/internal/fantasy"
)

// NewCastCallTool creates a new cast call tool
func NewCastCallTool() fantasy.AgentTool {
	return fantasy.NewAgentToolWithAllParams(
		CastCallToolName,
		"Call smart contract functions using Cast (read-only operations)",
		"Use this tool to call view/pure functions on smart contracts. Examples: checking balances, reading contract state, calling getter functions.",
		"",
		func(ctx context.Context, toolInput *fantasy.ToolInput) (*fantasy.ToolResult, error) {
			return CastCallTool(ctx, toolInput)
		},
	)
}

// NewCastSendTool creates a new cast send tool  
func NewCastSendTool() fantasy.AgentTool {
	return fantasy.NewAgentToolWithAllParams(
		CastSendToolName,
		"Send transactions using Cast (writes to blockchain)",
		"Use this tool to send transactions that modify blockchain state. Examples: transferring tokens, calling contract functions that change state, deploying contracts. ALWAYS ask for user confirmation before executing.",
		"Use this tool when the user wants to:\n- Send ETH or tokens to an address\n- Call contract functions that modify state (transfer, approve, mint, etc.)\n- Deploy contracts\n- Any operation that requires gas and writes to blockchain\n\nIMPORTANT: Always explain the transaction details and ask for confirmation before executing!",
		func(ctx context.Context, toolInput *fantasy.ToolInput) (*fantasy.ToolResult, error) {
			return CastSendTool(ctx, toolInput)
		},
	)
}

// NewGasPriceTool creates a new gas price tool
func NewGasPriceTool() fantasy.AgentTool {
	return fantasy.NewAgentToolWithAllParams(
		GasPriceToolName,
		"Check current gas prices using Cast",
		"Use this tool to check current gas prices on the configured network. This helps users optimize transaction costs.",
		"Use this tool when the user wants to:\n- Check current gas prices\n- Get gas price recommendations\n- Understand transaction costs\n- Optimize gas usage for transactions",
		func(ctx context.Context, toolInput *fantasy.ToolInput) (*fantasy.ToolResult, error) {
			return GasPriceTool(ctx, toolInput)
		},
	)
}