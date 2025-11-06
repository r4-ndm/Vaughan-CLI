package interfaces

// Core interfaces for Vaughan Crush modularity and maintainability

import (
	"context"
	"time"

	"github.com/r4v3n/vaughan-cli/internal/fantasy"
)

// BlockchainNetwork defines interface for blockchain network implementations
type BlockchainNetwork interface {
	GetName() string
	GetChainID() int
	GetRPCURL() string
	GetGasToken() string
	GetBlockTime() int
	GetExplorerURL() string
	GetType() string // "mainnet", "testnet", "local"
}

// ToolProvider defines interface for tool providers
type ToolProvider interface {
	GetName() string
	GetDescription() string
	GetTools() []fantasy.AgentTool
	RegisterTool(tool fantasy.AgentTool) error
	UnregisterTool(toolName string) error
}

// ConfigManager defines interface for configuration management
type ConfigManager interface {
	Load() error
	Save() error
	GetNetwork(networkName string) (BlockchainNetwork, error)
	GetNetworks() []BlockchainNetwork
	GetDefaultNetwork() string
	SetDefaultNetwork(networkName string) error
	Validate() error
}

// ModelProvider defines interface for AI model providers
type ModelProvider interface {
	GetName() string
	GetType() string // "local-ollama", "openai", "anthropic", etc.
	GetModel() string
	GetEndpoint() string
	Generate(ctx context.Context, prompt string) (string, error)
	IsAvailable() bool
}

// CastExecutor defines interface for Cast command execution
type CastExecutor interface {
	ExecuteCommand(ctx context.Context, command string, args []string, options map[string]string) (string, error)
	ValidateCommand(command string, args []string) error
	GetSupportedCommands() []string
}

// ErrorHandler defines interface for error handling
type ErrorHandler interface {
	HandleError(err error, context string) error
	LogError(err error, context string)
	GetUserMessage(err error) string
	ShouldRetry(err error) bool
}

// UpdateManager defines interface for managing updates from Crush
type UpdateManager interface {
	CheckForUpdates() (UpdateInfo, error)
	ApplyUpdate(updateInfo UpdateInfo) error
	GetCurrentVersion() string
	IsUpdateAvailable() bool
}

// UpdateInfo contains information about available updates
type UpdateInfo struct {
	Version      string
	ReleaseNotes string
	DownloadURL  string
	Critical     bool
}

// Validator defines interface for validation
type Validator interface {
	Validate(value interface{}) error
	GetValidationRules() []ValidationRule
}

// ValidationRule defines a validation rule
type ValidationRule struct {
	Name        string
	Description string
	Validate    func(interface{}) error
}

// Logger defines interface for logging
type Logger interface {
	Info(message string, args ...interface{})
	Debug(message string, args ...interface{})
	Error(message string, args ...interface{})
	Warn(message string, args ...interface{})
}

// SessionManager defines interface for session management
type SessionManager interface {
	CreateSession(userID string) (string, error)
	GetSession(sessionID string) (Session, error)
	UpdateSession(sessionID string, updates SessionUpdate) error
	DeleteSession(sessionID string) error
	ListSessions(userID string) ([]Session, error)
}

// Session represents a user session
type Session struct {
	ID        string
	UserID    string
	CreatedAt time.Time
	UpdatedAt time.Time
	Data      map[string]interface{}
}

// SessionUpdate contains updates for a session
type SessionUpdate struct {
	Network     *string
	Model       *string
	Preferences *map[string]interface{}
}