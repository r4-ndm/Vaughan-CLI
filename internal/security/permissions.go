package security

import (
	"fmt"
	"sync"
)

// Permission represents a system permission
type Permission string

const (
	// Basic permissions
	PermissionRead   Permission = "read"
	PermissionWrite  Permission = "write"
	PermissionExecute Permission = "execute"
	
	// Blockchain-specific permissions
	PermissionQuery     Permission = "query"       // Read blockchain data
	PermissionSign      Permission = "sign"        // Sign transactions
	PermissionSend      Permission = "send"        // Send transactions
	PermissionDeploy    Permission = "deploy"      // Deploy contracts
	PermissionImport    Permission = "import"      // Import wallet
	
	// System permissions
	PermissionConfig    Permission = "config"      // Modify configuration
	PermissionSystem    Permission = "system"      // System operations
	PermissionNetwork    Permission = "network"      // Network operations
)

// Context represents security context
type Context struct {
	UserID      string
	SessionID    string
	Permissions  []Permission
	NetworkAccess map[string]bool // Network name -> allowed
	TimeAllowed  bool            // Time-based restrictions
}

// PermissionManager manages tool permissions
type PermissionManager struct {
	toolPermissions map[string][]Permission
	defaultPerms    []Permission
	mutex           sync.RWMutex
}

// NewPermissionManager creates a new permission manager
func NewPermissionManager() *PermissionManager {
	return &PermissionManager{
		toolPermissions: make(map[string][]Permission),
		defaultPerms: []Permission{
			PermissionRead,
			PermissionQuery,
		},
	}
}

// SetToolPermissions sets permissions for a specific tool
func (pm *PermissionManager) SetToolPermissions(toolName string, permissions []Permission) {
	pm.mutex.Lock()
	defer pm.mutex.Unlock()
	
	pm.toolPermissions[toolName] = permissions
}

// GetToolPermissions returns permissions for a tool
func (pm *PermissionManager) GetToolPermissions(toolName string) []Permission {
	pm.mutex.RLock()
	defer pm.mutex.RUnlock()
	
	if perms, exists := pm.toolPermissions[toolName]; exists {
		return perms
	}
	
	return pm.defaultPerms
}

// CheckPermission checks if context has permission for tool
func (pm *PermissionManager) CheckPermission(ctx *Context, toolName string, requiredPerm Permission) bool {
	toolPerms := pm.GetToolPermissions(toolName)
	
	// Check if tool requires the permission
	hasToolPerm := false
	for _, perm := range toolPerms {
		if perm == requiredPerm {
			hasToolPerm = true
			break
		}
	}
	
	if !hasToolPerm {
		return false
	}
	
	// Check if user has the permission
	for _, userPerm := range ctx.Permissions {
		if userPerm == requiredPerm {
			return true
		}
	}
	
	return false
}

// CheckNetworkAccess checks if user can access specific network
func (pm *PermissionManager) CheckNetworkAccess(ctx *Context, networkName string) bool {
	if ctx.NetworkAccess == nil {
		return true // No restrictions
	}
	
	allowed, exists := ctx.NetworkAccess[networkName]
	return exists && allowed
}

// ValidateToolExecution validates if a tool can be executed
func (pm *PermissionManager) ValidateToolExecution(ctx *Context, toolName string, params map[string]interface{}) error {
	toolPerms := pm.GetToolPermissions(toolName)
	
	// Check each required permission
	for _, perm := range toolPerms {
		if !pm.CheckPermission(ctx, toolName, perm) {
			return fmt.Errorf("permission denied: %s required for tool %s", perm, toolName)
		}
	}
	
	// Special validation for sensitive operations
	switch toolName {
	case "cast_send":
		return pm.validateTransaction(ctx, params)
	case "cast_call":
		return pm.validateContractCall(ctx, params)
	case "hardware_wallet_import":
		return pm.validateWalletImport(ctx, params)
	}
	
	return nil
}

// validateTransaction validates transaction execution
func (pm *PermissionManager) validateTransaction(ctx *Context, params map[string]interface{}) error {
	if !pm.CheckPermission(ctx, "cast_send", PermissionSend) {
		return fmt.Errorf("permission denied: send permission required")
	}
	
	// Check network access
	if network, ok := params["network"].(string); ok {
		if !pm.CheckNetworkAccess(ctx, network) {
			return fmt.Errorf("permission denied: no access to network %s", network)
		}
	}
	
	return nil
}

// validateContractCall validates contract call
func (pm *PermissionManager) validateContractCall(ctx *Context, params map[string]interface{}) error {
	if !pm.CheckPermission(ctx, "cast_call", PermissionQuery) {
		return fmt.Errorf("permission denied: query permission required")
	}
	
	return nil
}

// validateWalletImport validates wallet import
func (pm *PermissionManager) validateWalletImport(ctx *Context, params map[string]interface{}) error {
	if !pm.CheckPermission(ctx, "hardware_wallet_import", PermissionImport) {
		return fmt.Errorf("permission denied: import permission required")
	}
	
	return nil
}

// CreateSecureContext creates a secure context for system operations
func (pm *PermissionManager) CreateSecureContext(userID, sessionID string) *Context {
	return &Context{
		UserID:      userID,
		SessionID:    sessionID,
		Permissions:  pm.defaultPerms,
		NetworkAccess: make(map[string]bool),
		TimeAllowed:  true,
	}
}

// CreateUnrestrictedContext creates context for admin operations
func (pm *PermissionManager) CreateUnrestrictedContext(userID, sessionID string) *Context {
	return &Context{
		UserID:      userID,
		SessionID:    sessionID,
		Permissions: []Permission{
			PermissionRead,
			PermissionWrite,
			PermissionExecute,
			PermissionQuery,
			PermissionSign,
			PermissionSend,
			PermissionDeploy,
			PermissionImport,
			PermissionConfig,
			PermissionSystem,
			PermissionNetwork,
		},
		NetworkAccess: nil, // No restrictions
		TimeAllowed:  true,
	}
}

// Default tool permissions
var DefaultToolPermissions = map[string][]Permission{
	"cast_balance":    {PermissionQuery},
	"cast_call":       {PermissionQuery},
	"cast_send":       {PermissionSign, PermissionSend},
	"gas_price":       {PermissionQuery},
	"view":           {PermissionRead},
	"ls":             {PermissionRead},
	"grep":           {PermissionRead},
	"bash":           {PermissionExecute},
	"download":       {PermissionRead, PermissionNetwork},
	"fetch":          {PermissionRead, PermissionNetwork},
	"cast_estimate":   {PermissionQuery},
	"hardware_wallet_connect":    {PermissionRead},
	"hardware_wallet_import":      {PermissionImport},
	"hardware_wallet_sign":        {PermissionSign},
}

// Initialize default permissions
func (pm *PermissionManager) InitializeDefaults() {
	pm.mutex.Lock()
	defer pm.mutex.Unlock()
	
	for toolName, perms := range DefaultToolPermissions {
		pm.toolPermissions[toolName] = perms
	}
}
