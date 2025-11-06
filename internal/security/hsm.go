package security

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"sync"
	"time"
)

// HardwareSecurityModule provides HSM integration for key management
type HardwareSecurityModule struct {
	provider     HSMProvider
	initialized  bool
	config       *HSMConfig
	keyStore     map[string]*HSMKey
	sessions     map[string]*HSMSession
	mutex        sync.RWMutex
	logger       *SecurityLogger
	backup       *HSMBackup
	audit        *HSMAudit
}

// HSMProvider represents different HSM providers
type HSMProvider string

const (
	HSMProviderSoftHSM      HSMProvider = "softhsm"
	HSMProviderAWS           HSMProvider = "aws"
	HSMProviderAzure         HSMProvider = "azure"
	HSMProviderGoogleCloud   HSMProvider = "gcp"
	HSMProviderGemalto       HSMProvider = "gemalto"
	HSMProviderThales        HSMProvider = "thales"
	HSMProviderNitrokey      HSMProvider = "nitrokey"
	HSMProviderYubiHSM       HSMProvider = "yubihsm"
)

// HSMConfig represents HSM configuration
type HSMConfig struct {
	Provider        HSMProvider       `json:"provider"`
	Endpoint       string            `json:"endpoint"`
	Token          string            `json:"token,omitempty"`
	Pin            string            `json:"pin,omitempty"`
	Slot           int               `json:"slot"`
	KeySize        int               `json:"key_size"`
	HashAlgorithm  string            `json:"hash_algorithm"`
	EncryptionAlgo string            `json:"encryption_algorithm"`
	SignatureAlgo  string           `json:"signature_algorithm"`
	BackupEnabled  bool              `json:"backup_enabled"`
	BackupInterval  time.Duration     `json:"backup_interval"`
	MinKeyStrength  int              `json:"min_key_strength"`
	KeyRotation     time.Duration     `json:"key_rotation"`
	SessionTimeout  time.Duration     `json:"session_timeout"`
	MaxRetries      int               `json:"max_retries"`
	RetryDelay      time.Duration     `json:"retry_delay"`
}

// HSMKey represents keys stored in HSM
type HSMKey struct {
	ID              string            `json:"id"`
	Label           string            `json:"label"`
	Type            HSMKeyType       `json:"type"`
	Size            int               `json:"size"`
	CreatedAt       time.Time         `json:"created_at"`
	LastRotated     *time.Time        `json:"last_rotated,omitempty"`
	ExpiresAt       *time.Time        `json:"expires_at,omitempty"`
	Algorithm       string            `json:"algorithm"`
	Purpose         []string          `json:"purpose"`
	AccessLevel     HSMKeyAccess     `json:"access_level"`
	Metadata        map[string]string `json:"metadata"`
	Checksum        string            `json:"checksum"`
	UsageCount      int64             `json:"usage_count"`
	LastUsed        *time.Time        `json:"last_used,omitempty"`
	Disabled        bool              `json:"disabled"`
}

// HSMKeyType represents different key types
type HSMKeyType string

const (
	HSMKeyRSA      HSMKeyType = "RSA"
	HSMKeyECDSA    HSMKeyType = "ECDSA"
	HSMKeyAES      HSMKeyType = "AES"
	HSMKeyHMAC     HSMKeyType = "HMAC"
	HSMKeyDH       HSMKeyType = "DH"
)

// HSMKeyAccess represents key access levels
type HSMKeyAccess string

const (
	HSMKeyAccessPublic   HSMKeyAccess = "public"
	HSMKeyAccessPrivate  HSMKeyAccess = "private"
	HSMKeyAccessSecret   HSMKeyAccess = "secret"
)

// HSMSession represents HSM sessions
type HSMSession struct {
	ID              string     `json:"id"`
	UserID          string     `json:"user_id"`
	SessionHandle   string     `json:"session_handle"`
	CreatedAt       time.Time  `json:"created_at"`
	LastActive      time.Time  `json:"last_active"`
	ExpiresAt       time.Time  `json:"expires_at"`
	Permissions     []string   `json:"permissions"`
	Operations      []HSMOperation `json:"operations"`
	AccessCount     int        `json:"access_count"`
	Active          bool       `json:"active"`
}

// HSMOperation represents HSM operations
type HSMOperation struct {
	Type        string                 `json:"type"`
	KeyID       string                 `json:"key_id"`
	Timestamp   time.Time              `json:"timestamp"`
	UserID      string                 `json:"user_id"`
	SessionID   string                 `json:"session_id"`
	Input       map[string]interface{} `json:"input"`
	Output      map[string]interface{} `json:"output"`
	Success     bool                   `json:"success"`
	Error       string                 `json:"error,omitempty"`
	Duration    time.Duration          `json:"duration"`
	Metadata    map[string]string      `json:"metadata"`
}

// HSMBackup represents HSM backup configuration
type HSMBackup struct {
	Enabled      bool          `json:"enabled"`
	Location     string        `json:"location"`
	Encryption   bool          `json:"encryption"`
	Compression  bool          `json:"compression"`
	Schedule     time.Duration `json:"schedule"`
	Retention    time.Duration `json:"retention"`
	Destinations []BackupDestination `json:"destinations"`
	LastBackup   *time.Time    `json:"last_backup,omitempty"`
}

// BackupDestination represents backup storage destinations
type BackupDestination struct {
	Type        string            `json:"type"`
	Location    string            `json:"location"`
	Credentials map[string]string `json:"credentials"`
	Enabled     bool              `json:"enabled"`
	Priority    int               `json:"priority"`
}

// HSMAudit represents HSM audit trail
type HSMAudit struct {
	Enabled        bool              `json:"enabled"`
	LogAll         bool              `json:"log_all"`
	LogSuccess      bool              `json:"log_success"`
	LogFailures     bool              `json:"log_failures"`
	Retention      time.Duration     `json:"retention"`
	AlertThreshold map[string]int    `json:"alert_threshold"`
	Notifications  []AuditNotification `json:"notifications"`
}

// AuditNotification represents audit notifications
type AuditNotification struct {
	Type        string                 `json:"type"`
	Threshold   int                    `json:"threshold"`
	Window      time.Duration          `json:"window"`
	Destination map[string]interface{} `json:"destination"`
	Enabled     bool                   `json:"enabled"`
}

// KeyGenerationRequest represents HSM key generation request
type KeyGenerationRequest struct {
	Type            HSMKeyType       `json:"type"`
	Size            int               `json:"size"`
	Label           string            `json:"label"`
	Algorithm       string            `json:"algorithm"`
	Purpose         []string          `json:"purpose"`
	AccessLevel     HSMKeyAccess     `json:"access_level"`
	Metadata        map[string]string `json:"metadata"`
	Rotation        bool              `json:"rotation"`
	RotationPeriod  time.Duration     `json:"rotation_period"`
}

// SignatureRequest represents HSM signature request
type SignatureRequest struct {
	KeyID        string            `json:"key_id"`
	Algorithm    string            `json:"algorithm"`
	Hash         string            `json:"hash,omitempty"`
	Data         []byte            `json:"data"`
	Context      map[string]string `json:"context,omitempty"`
	SessionID    string            `json:"session_id"`
	UserID       string            `json:"user_id"`
	Timestamp    time.Time         `json:"timestamp"`
}

// EncryptionRequest represents HSM encryption request
type EncryptionRequest struct {
	KeyID        string            `json:"key_id"`
	Algorithm    string            `json:"algorithm"`
	Data         []byte            `json:"data"`
	IV           []byte            `json:"iv,omitempty"`
	Context      map[string]string `json:"context,omitempty"`
	SessionID    string            `json:"session_id"`
	UserID       string            `json:"user_id"`
	Timestamp    time.Time         `json:"timestamp"`
}

// NewHardwareSecurityModule creates a new HSM instance
func NewHardwareSecurityModule(config *HSMConfig, logger *SecurityLogger) *HardwareSecurityModule {
	hsm := &HardwareSecurityModule{
		provider:    config.Provider,
		config:      config,
		keyStore:    make(map[string]*HSMKey),
		sessions:    make(map[string]*HSMSession),
		logger:      logger,
		backup:      &HSMBackup{
			Enabled:   config.BackupEnabled,
			Schedule:  config.BackupInterval,
			Retention: 30 * 24 * time.Hour, // 30 days
		},
		audit: &HSMAudit{
			Enabled:    true,
			LogAll:     true,
			LogSuccess: true,
			LogFailures: true,
			Retention: 90 * 24 * time.Hour, // 90 days
		},
	}
	
	return hsm
}

// Initialize initializes the HSM
func (hsm *HardwareSecurityModule) Initialize() error {
	hsm.mutex.Lock()
	defer hsm.mutex.Unlock()
	
	// Initialize HSM provider
	switch hsm.provider {
	case HSMProviderSoftHSM:
		if err := hsm.initializeSoftHSM(); err != nil {
			return fmt.Errorf("failed to initialize SoftHSM: %w", err)
		}
	case HSMProviderAWS:
		if err := hsm.initializeAWSHSM(); err != nil {
			return fmt.Errorf("failed to initialize AWS HSM: %w", err)
		}
	case HSMProviderAzure:
		if err := hsm.initializeAzureHSM(); err != nil {
			return fmt.Errorf("failed to initialize Azure HSM: %w", err)
		}
	default:
		return fmt.Errorf("unsupported HSM provider: %s", hsm.provider)
	}
	
	// Start background services
	go hsm.startBackgroundServices()
	
	// Log initialization
	if hsm.logger != nil {
		hsm.logger.LogHSMEvent("hsm_initialized", fmt.Sprintf("HSM initialized with provider: %s", hsm.provider), map[string]interface{}{
			"provider": hsm.provider,
			"slot":     hsm.config.Slot,
		})
	}
	
	hsm.initialized = true
	return nil
}

// CreateSession creates a new HSM session
func (hsm *HardwareSecurityModule) CreateSession(userID string, permissions []string) (*HSMSession, error) {
	if !hsm.initialized {
		return nil, fmt.Errorf("HSM not initialized")
	}
	
	hsm.mutex.Lock()
	defer hsm.mutex.Unlock()
	
	// Create session handle
	sessionHandle, err := hsm.createSessionHandle(userID)
	if err != nil {
		return nil, fmt.Errorf("failed to create session handle: %w", err)
	}
	
	// Create session
	session := &HSMSession{
		ID:            hsm.generateSessionID(),
		UserID:        userID,
		SessionHandle: sessionHandle,
		CreatedAt:     time.Now(),
		LastActive:    time.Now(),
		ExpiresAt:     time.Now().Add(hsm.config.SessionTimeout),
		Permissions:   permissions,
		Operations:    make([]HSMOperation, 0),
		AccessCount:   0,
		Active:        true,
	}
	
	hsm.sessions[session.ID] = session
	
	// Log session creation
	if hsm.logger != nil {
		hsm.logger.LogHSMEvent("session_created", "HSM session created", map[string]interface{}{
			"session_id":  session.ID,
			"user_id":     userID,
			"permissions": permissions,
		})
	}
	
	return session, nil
}

// CloseSession closes an HSM session
func (hsm *HardwareSecurityModule) CloseSession(sessionID string) error {
	hsm.mutex.Lock()
	defer hsm.mutex.Unlock()
	
	session, exists := hsm.sessions[sessionID]
	if !exists {
		return fmt.Errorf("session not found: %s", sessionID)
	}
	
	// Close session handle
	if err := hsm.closeSessionHandle(session.SessionHandle); err != nil {
		return fmt.Errorf("failed to close session handle: %w", err)
	}
	
	session.Active = false
	
	// Log session closure
	if hsm.logger != nil {
		hsm.logger.LogHSMEvent("session_closed", "HSM session closed", map[string]interface{}{
			"session_id":   sessionID,
			"user_id":      session.UserID,
			"access_count": session.AccessCount,
			"duration":     time.Since(session.CreatedAt),
		})
	}
	
	return nil
}

// GenerateKey generates a new key in HSM
func (hsm *HardwareSecurityModule) GenerateKey(request *KeyGenerationRequest, sessionID string) (*HSMKey, error) {
	if !hsm.initialized {
		return nil, fmt.Errorf("HSM not initialized")
	}
	
	// Validate session
	session, err := hsm.validateSession(sessionID, "key_generate")
	if err != nil {
		return nil, err
	}
	
	// Validate request
	if err := hsm.validateKeyGenerationRequest(request); err != nil {
		return nil, fmt.Errorf("invalid key generation request: %w", err)
	}
	
	// Generate key
	key, err := hsm.generateKeyInHSM(request, session)
	if err != nil {
		// Log failed operation
		hsm.logOperation(&HSMOperation{
			Type:      "key_generate",
			SessionID: sessionID,
			UserID:    session.UserID,
			Timestamp: time.Now(),
			Input: map[string]interface{}{
				"request": request,
			},
			Success: false,
			Error:    err.Error(),
		})
		
		return nil, fmt.Errorf("failed to generate key: %w", err)
	}
	
	// Store key metadata
	hsm.keyStore[key.ID] = key
	
	// Log successful operation
	hsm.logOperation(&HSMOperation{
		Type:      "key_generate",
		KeyID:     key.ID,
		SessionID: sessionID,
		UserID:    session.UserID,
		Timestamp: time.Now(),
		Input: map[string]interface{}{
			"request": request,
		},
		Output: map[string]interface{}{
			"key_id": key.ID,
		},
		Success: true,
		Duration: 0, // TODO: measure actual duration
	})
	
	return key, nil
}

// Sign signs data using HSM
func (hsm *HardwareSecurityModule) Sign(request *SignatureRequest, sessionID string) ([]byte, error) {
	if !hsm.initialized {
		return nil, fmt.Errorf("HSM not initialized")
	}
	
	// Validate session
	session, err := hsm.validateSession(sessionID, "sign")
	if err != nil {
		return nil, err
	}
	
	// Get key
	key, exists := hsm.keyStore[request.KeyID]
	if !exists || key.Disabled {
		return nil, fmt.Errorf("key not found or disabled: %s", request.KeyID)
	}
	
	// Validate key type for signing
	if !hsm.isSigningKey(key.Type) {
		return nil, fmt.Errorf("key type not suitable for signing: %s", key.Type)
	}
	
	// Perform signature
	signature, err := hsm.signInHSM(request, key, session)
	if err != nil {
		// Log failed operation
		hsm.logOperation(&HSMOperation{
			Type:      "sign",
			KeyID:     request.KeyID,
			SessionID: sessionID,
			UserID:    session.UserID,
			Timestamp: time.Now(),
			Input: map[string]interface{}{
				"algorithm": request.Algorithm,
				"data_size": len(request.Data),
			},
			Success: false,
			Error:    err.Error(),
		})
		
		return nil, fmt.Errorf("failed to sign data: %w", err)
	}
	
	// Update key usage
	key.UsageCount++
	now := time.Now()
	key.LastUsed = &now
	
	// Log successful operation
	hsm.logOperation(&HSMOperation{
		Type:      "sign",
		KeyID:     request.KeyID,
		SessionID: sessionID,
		UserID:    session.UserID,
		Timestamp: time.Now(),
		Input: map[string]interface{}{
			"algorithm": request.Algorithm,
			"data_size": len(request.Data),
		},
		Output: map[string]interface{}{
			"signature_size": len(signature),
		},
		Success: true,
		Duration: 0, // TODO: measure actual duration
	})
	
	return signature, nil
}

// Encrypt encrypts data using HSM
func (hsm *HardwareSecurityModule) Encrypt(request *EncryptionRequest, sessionID string) ([]byte, error) {
	if !hsm.initialized {
		return nil, fmt.Errorf("HSM not initialized")
	}
	
	// Validate session
	session, err := hsm.validateSession(sessionID, "encrypt")
	if err != nil {
		return nil, err
	}
	
	// Get key
	key, exists := hsm.keyStore[request.KeyID]
	if !exists || key.Disabled {
		return nil, fmt.Errorf("key not found or disabled: %s", request.KeyID)
	}
	
	// Validate key type for encryption
	if !hsm.isEncryptionKey(key.Type) {
		return nil, fmt.Errorf("key type not suitable for encryption: %s", key.Type)
	}
	
	// Perform encryption
	ciphertext, err := hsm.encryptInHSM(request, key, session)
	if err != nil {
		// Log failed operation
		hsm.logOperation(&HSMOperation{
			Type:      "encrypt",
			KeyID:     request.KeyID,
			SessionID: sessionID,
			UserID:    session.UserID,
			Timestamp: time.Now(),
			Input: map[string]interface{}{
				"algorithm": request.Algorithm,
				"data_size": len(request.Data),
			},
			Success: false,
			Error:    err.Error(),
		})
		
		return nil, fmt.Errorf("failed to encrypt data: %w", err)
	}
	
	// Update key usage
	key.UsageCount++
	now := time.Now()
	key.LastUsed = &now
	
	// Log successful operation
	hsm.logOperation(&HSMOperation{
		Type:      "encrypt",
		KeyID:     request.KeyID,
		SessionID: sessionID,
		UserID:    session.UserID,
		Timestamp: time.Now(),
		Input: map[string]interface{}{
			"algorithm": request.Algorithm,
			"data_size": len(request.Data),
		},
		Output: map[string]interface{}{
			"ciphertext_size": len(ciphertext),
		},
		Success: true,
		Duration: 0, // TODO: measure actual duration
	})
	
	return ciphertext, nil
}

// DeleteKey deletes a key from HSM
func (hsm *HardwareSecurityModule) DeleteKey(keyID, sessionID string) error {
	if !hsm.initialized {
		return fmt.Errorf("HSM not initialized")
	}
	
	// Validate session
	session, err := hsm.validateSession(sessionID, "key_delete")
	if err != nil {
		return err
	}
	
	// Get key
	key, exists := hsm.keyStore[keyID]
	if !exists {
		return fmt.Errorf("key not found: %s", keyID)
	}
	
	// Delete key from HSM
	if err := hsm.deleteKeyFromHSM(keyID, session); err != nil {
		// Log failed operation
		hsm.logOperation(&HSMOperation{
			Type:      "key_delete",
			KeyID:     keyID,
			SessionID: sessionID,
			UserID:    session.UserID,
			Timestamp: time.Now(),
			Input: map[string]interface{}{
				"key_label": key.Label,
			},
			Success: false,
			Error:    err.Error(),
		})
		
		return fmt.Errorf("failed to delete key: %w", err)
	}
	
	// Remove from key store
	delete(hsm.keyStore, keyID)
	
	// Log successful operation
	hsm.logOperation(&HSMOperation{
		Type:      "key_delete",
		KeyID:     keyID,
		SessionID: sessionID,
		UserID:    session.UserID,
		Timestamp: time.Now(),
		Input: map[string]interface{}{
			"key_label": key.Label,
		},
		Success: true,
		Duration: 0,
	})
	
	return nil
}

// RotateKey rotates an existing key
func (hsm *HardwareSecurityModule) RotateKey(keyID, sessionID string) (*HSMKey, error) {
	if !hsm.initialized {
		return nil, fmt.Errorf("HSM not initialized")
	}
	
	// Validate session
	session, err := hsm.validateSession(sessionID, "key_rotate")
	if err != nil {
		return nil, err
	}
	
	// Get existing key
	oldKey, exists := hsm.keyStore[keyID]
	if !exists {
		return nil, fmt.Errorf("key not found: %s", keyID)
	}
	
	// Generate new key with same parameters
	request := &KeyGenerationRequest{
		Type:        oldKey.Type,
		Size:        oldKey.Size,
		Label:       fmt.Sprintf("%s_rotated", oldKey.Label),
		Algorithm:   oldKey.Algorithm,
		Purpose:     oldKey.Purpose,
		AccessLevel: oldKey.AccessLevel,
		Metadata:    oldKey.Metadata,
		Rotation:    oldKey.Metadata["rotation"] == "true",
	}
	
	newKey, err := hsm.GenerateKey(request, sessionID)
	if err != nil {
		return nil, fmt.Errorf("failed to generate new key for rotation: %w", err)
	}
	
	// Mark old key for retirement (don't delete immediately)
	oldKey.ExpiresAt = &time.Time{} // TODO: set appropriate retirement time
	
	// Log key rotation
	if hsm.logger != nil {
		hsm.logger.LogHSMEvent("key_rotated", "HSM key rotated", map[string]interface{}{
			"old_key_id": keyID,
			"new_key_id": newKey.ID,
			"old_label":  oldKey.Label,
			"new_label":  newKey.Label,
		})
	}
	
	return newKey, nil
}

// GetKey returns key metadata
func (hsm *HardwareSecurityModule) GetKey(keyID, sessionID string) (*HSMKey, error) {
	if !hsm.initialized {
		return nil, fmt.Errorf("HSM not initialized")
	}
	
	// Validate session
	_, err := hsm.validateSession(sessionID, "key_get")
	if err != nil {
		return nil, err
	}
	
	// Get key
	key, exists := hsm.keyStore[keyID]
	if !exists {
		return nil, fmt.Errorf("key not found: %s", keyID)
	}
	
	// Return metadata only (not actual key material)
	return key, nil
}

// ListKeys returns list of keys
func (hsm *HardwareSecurityModule) ListKeys(sessionID string, filter map[string]string) ([]*HSMKey, error) {
	if !hsm.initialized {
		return nil, fmt.Errorf("HSM not initialized")
	}
	
	// Validate session
	_, err := hsm.validateSession(sessionID, "key_list")
	if err != nil {
		return nil, err
	}
	
	// Filter keys
	var keys []*HSMKey
	for _, key := range hsm.keyStore {
		if hsm.matchesKeyFilter(key, filter) {
			keys = append(keys, key)
		}
	}
	
	return keys, nil
}

// GetSession returns session information
func (hsm *HardwareSecurityModule) GetSession(sessionID string) (*HSMSession, error) {
	if !hsm.initialized {
		return nil, fmt.Errorf("HSM not initialized")
	}
	
	hsm.mutex.RLock()
	defer hsm.mutex.RUnlock()
	
	session, exists := hsm.sessions[sessionID]
	if !exists {
		return nil, fmt.Errorf("session not found: %s", sessionID)
	}
	
	return session, nil
}

// GetStatistics returns HSM statistics
func (hsm *HardwareSecurityModule) GetStatistics() HSMStatistics {
	hsm.mutex.RLock()
	defer hsm.mutex.RUnlock()
	
	stats := HSMStatistics{
		TotalKeys:         len(hsm.keyStore),
		ActiveKeys:        0,
		ExpiredKeys:       0,
		DisabledKeys:      0,
		ActiveSessions:    0,
		TotalOperations:   0,
		SuccessfulOps:     0,
		FailedOps:         0,
		KeysByType:        make(map[HSMKeyType]int),
		KeysByAccessLevel: make(map[HSMKeyAccess]int),
		OperationsByType:  make(map[string]int),
	}
	
	now := time.Now()
	
	// Count keys
	for _, key := range hsm.keyStore {
		if key.Disabled {
			stats.DisabledKeys++
		} else if key.ExpiresAt != nil && key.ExpiresAt.Before(now) {
			stats.ExpiredKeys++
		} else {
			stats.ActiveKeys++
		}
		
		stats.KeysByType[key.Type]++
		stats.KeysByAccessLevel[key.AccessLevel]++
	}
	
	// Count sessions
	for _, session := range hsm.sessions {
		if session.Active && session.ExpiresAt.After(now) {
			stats.ActiveSessions++
		}
		
		// Count operations
		stats.TotalOperations += len(session.Operations)
		for _, op := range session.Operations {
			if op.Success {
				stats.SuccessfulOps++
			} else {
				stats.FailedOps++
			}
			stats.OperationsByType[op.Type]++
		}
	}
	
	return stats
}

// PerformBackup performs HSM backup
func (hsm *HardwareSecurityModule) PerformBackup() error {
	if !hsm.backup.Enabled {
		return fmt.Errorf("backup is not enabled")
	}
	
	// Create backup
	backupData, err := hsm.createBackupData()
	if err != nil {
		return fmt.Errorf("failed to create backup data: %w", err)
	}
	
	// Store backup to destinations
	for _, dest := range hsm.backup.Destinations {
		if dest.Enabled {
			if err := hsm.storeBackup(dest, backupData); err != nil {
				return fmt.Errorf("failed to store backup to %s: %w", dest.Location, err)
			}
		}
	}
	
	// Update backup timestamp
	now := time.Now()
	hsm.backup.LastBackup = &now
	
	// Log backup
	if hsm.logger != nil {
		hsm.logger.LogHSMEvent("backup_completed", "HSM backup completed", map[string]interface{}{
			"backup_size":   len(backupData),
			"destinations":  len(hsm.backup.Destinations),
		})
	}
	
	return nil
}

// validateSession validates session and permissions
func (hsm *HardwareSecurityModule) validateSession(sessionID, operation string) (*HSMSession, error) {
	hsm.mutex.RLock()
	defer hsm.mutex.RUnlock()
	
	session, exists := hsm.sessions[sessionID]
	if !exists {
		return nil, fmt.Errorf("session not found: %s", sessionID)
	}
	
	if !session.Active {
		return nil, fmt.Errorf("session is not active: %s", sessionID)
	}
	
	if time.Now().After(session.ExpiresAt) {
		return nil, fmt.Errorf("session has expired: %s", sessionID)
	}
	
	// Check permissions (simplified)
	hasPermission := false
	for _, perm := range session.Permissions {
		if perm == "all" || perm == operation {
			hasPermission = true
			break
		}
	}
	
	if !hasPermission {
		return nil, fmt.Errorf("session does not have permission for operation: %s", operation)
	}
	
	// Update session activity
	session.LastActive = time.Now()
	session.AccessCount++
	
	return session, nil
}

// validateKeyGenerationRequest validates key generation request
func (hsm *HardwareSecurityModule) validateKeyGenerationRequest(request *KeyGenerationRequest) error {
	// Validate key type
	switch request.Type {
	case HSMKeyRSA:
		if request.Size < 2048 {
			return fmt.Errorf("RSA key size must be at least 2048 bits")
		}
	case HSMKeyECDSA:
		if request.Size < 256 {
			return fmt.Errorf("ECDSA key size must be at least 256 bits")
		}
	case HSMKeyAES:
		if request.Size != 128 && request.Size != 256 && request.Size != 512 {
			return fmt.Errorf("AES key size must be 128, 256, or 512 bits")
		}
	default:
		return fmt.Errorf("unsupported key type: %s", request.Type)
	}
	
	// Validate against minimum key strength
	if request.Size < hsm.config.MinKeyStrength {
		return fmt.Errorf("key size below minimum strength: %d < %d", request.Size, hsm.config.MinKeyStrength)
	}
	
	return nil
}

// isSigningKey checks if key type is suitable for signing
func (hsm *HardwareSecurityModule) isSigningKey(keyType HSMKeyType) bool {
	return keyType == HSMKeyRSA || keyType == HSMKeyECDSA || keyType == HSMKeyHMAC
}

// isEncryptionKey checks if key type is suitable for encryption
func (hsm *HardwareSecurityModule) isEncryptionKey(keyType HSMKeyType) bool {
	return keyType == HSMKeyAES || keyType == HSMKeyRSA
}

// matchesKeyFilter checks if key matches filter criteria
func (hsm *HardwareSecurityModule) matchesKeyFilter(key *HSMKey, filter map[string]string) bool {
	if len(filter) == 0 {
		return true
	}
	
	for keyName, value := range filter {
		switch keyName {
		case "type":
			if string(key.Type) != value {
				return false
			}
		case "label":
			if key.Label != value {
				return false
			}
		case "access_level":
			if string(key.AccessLevel) != value {
				return false
			}
		case "disabled":
			disabled := "false"
			if key.Disabled {
				disabled = "true"
			}
			if disabled != value {
				return false
			}
		}
	}
	
	return true
}

// logOperation logs HSM operation
func (hsm *HardwareSecurityModule) logOperation(operation *HSMOperation) {
	// Add to session operations
	if session, exists := hsm.sessions[operation.SessionID]; exists {
		session.Operations = append(session.Operations, *operation)
		
		// Limit operations in session (keep last 1000)
		if len(session.Operations) > 1000 {
			session.Operations = session.Operations[1:]
		}
	}
	
	// Log to security logger
	if hsm.logger != nil {
		hsm.logger.LogHSMOperation(operation)
	}
}

// startBackgroundServices starts HSM background services
func (hsm *HardwareSecurityModule) startBackgroundServices() {
	// Session cleanup
	go func() {
		ticker := time.NewTicker(5 * time.Minute)
		defer ticker.Stop()
		
		for {
			select {
			case <-ticker.C:
				hsm.cleanupExpiredSessions()
			}
		}
	}()
	
	// Key rotation
	go func() {
		ticker := time.NewTicker(hsm.config.KeyRotation)
		defer ticker.Stop()
		
		for {
			select {
			case <-ticker.C:
				hsm.checkKeyRotations()
			}
		}
	}()
	
	// Backup service
	if hsm.backup.Enabled {
		go func() {
			ticker := time.NewTicker(hsm.backup.Schedule)
			defer ticker.Stop()
			
			for {
				select {
				case <-ticker.C:
					hsm.PerformBackup()
				}
			}
		}()
	}
}

// cleanupExpiredSessions removes expired sessions
func (hsm *HardwareSecurityModule) cleanupExpiredSessions() {
	hsm.mutex.Lock()
	defer hsm.mutex.Unlock()
	
	now := time.Now()
	for sessionID, session := range hsm.sessions {
		if session.ExpiresAt.Before(now) {
			hsm.closeSessionHandle(session.SessionHandle)
			delete(hsm.sessions, sessionID)
			
			// Log session expiry
			if hsm.logger != nil {
				hsm.logger.LogHSMEvent("session_expired", "HSM session expired", map[string]interface{}{
					"session_id":   sessionID,
					"user_id":      session.UserID,
					"access_count": session.AccessCount,
				})
			}
		}
	}
}

// checkKeyRotations checks for keys requiring rotation
func (hsm *HardwareSecurityModule) checkKeyRotations() {
	hsm.mutex.Lock()
	defer hsm.mutex.Unlock()
	
	now := time.Now()
	for keyID, key := range hsm.keyStore {
		if key.Disabled {
			continue
		}
		
		// Check if key needs rotation
		needsRotation := false
		if rotationPeriod, exists := key.Metadata["rotation_period"]; exists {
			// Parse rotation period (simplified)
			if rotationPeriod == "1year" {
				needsRotation = time.Since(key.CreatedAt) > 365*24*time.Hour
			} else if rotationPeriod == "6months" {
				needsRotation = time.Since(key.CreatedAt) > 6*30*24*time.Hour
			}
		}
		
		if needsRotation {
			// Log key rotation requirement
			if hsm.logger != nil {
				hsm.logger.LogHSMEvent("key_rotation_required", "Key rotation required", map[string]interface{}{
					"key_id":    keyID,
					"key_label": key.Label,
					"created_at": key.CreatedAt,
				})
			}
		}
	}
}

// HSM provider implementations (simplified placeholders)

func (hsm *HardwareSecurityModule) initializeSoftHSM() error {
	// Initialize SoftHSM
	// This is a placeholder implementation
	return nil
}

func (hsm *HardwareSecurityModule) initializeAWSHSM() error {
	// Initialize AWS CloudHSM
	// This is a placeholder implementation
	return nil
}

func (hsm *HardwareSecurityModule) initializeAzureHSM() error {
	// Initialize Azure Dedicated HSM
	// This is a placeholder implementation
	return nil
}

func (hsm *HardwareSecurityModule) createSessionHandle(userID string) (string, error) {
	// Create session handle in HSM
	// This is a placeholder implementation
	return fmt.Sprintf("session_%s_%d", userID, time.Now().UnixNano()), nil
}

func (hsm *HardwareSecurityModule) closeSessionHandle(handle string) error {
	// Close session handle in HSM
	// This is a placeholder implementation
	return nil
}

func (hsm *HardwareSecurityModule) generateKeyInHSM(request *KeyGenerationRequest, session *HSMSession) (*HSMKey, error) {
	// Generate key in HSM
	// This is a placeholder implementation
	keyID := hsm.generateKeyID()
	
	return &HSMKey{
		ID:          keyID,
		Label:       request.Label,
		Type:        request.Type,
		Size:        request.Size,
		CreatedAt:   time.Now(),
		Algorithm:   request.Algorithm,
		Purpose:     request.Purpose,
		AccessLevel: request.AccessLevel,
		Metadata:    request.Metadata,
		Checksum:    hsm.calculateKeyChecksum(keyID),
		UsageCount:  0,
		Disabled:    false,
	}, nil
}

func (hsm *HardwareSecurityModule) signInHSM(request *SignatureRequest, key *HSMKey, session *HSMSession) ([]byte, error) {
	// Sign data in HSM
	// This is a placeholder implementation
	signature := make([]byte, 256) // Placeholder signature size
	rand.Read(signature)
	return signature, nil
}

func (hsm *HardwareSecurityModule) encryptInHSM(request *EncryptionRequest, key *HSMKey, session *HSMSession) ([]byte, error) {
	// Encrypt data in HSM
	// This is a placeholder implementation
	ciphertext := make([]byte, len(request.Data)+16) // Placeholder ciphertext with padding
	copy(ciphertext, request.Data)
	return ciphertext, nil
}

func (hsm *HardwareSecurityModule) deleteKeyFromHSM(keyID string, session *HSMSession) error {
	// Delete key from HSM
	// This is a placeholder implementation
	return nil
}

func (hsm *HardwareSecurityModule) createBackupData() ([]byte, error) {
	// Create backup data from keys
	// This is a placeholder implementation
	return []byte("backup_data_placeholder"), nil
}

func (hsm *HardwareSecurityModule) storeBackup(dest BackupDestination, data []byte) error {
	// Store backup to destination
	// This is a placeholder implementation
	return nil
}

// Utility functions

func (hsm *HardwareSecurityModule) generateKeyID() string {
	bytes := make([]byte, 16)
	rand.Read(bytes)
	return hex.EncodeToString(bytes)
}

func (hsm *HardwareSecurityModule) generateSessionID() string {
	bytes := make([]byte, 12)
	rand.Read(bytes)
	return hex.EncodeToString(bytes)
}

func (hsm *HardwareSecurityModule) calculateKeyChecksum(keyID string) string {
	hash := sha256.Sum256([]byte(keyID))
	return hex.EncodeToString(hash[:])
}

// HSMStatistics represents HSM usage statistics
type HSMStatistics struct {
	TotalKeys         int                      `json:"total_keys"`
	ActiveKeys        int                      `json:"active_keys"`
	ExpiredKeys       int                      `json:"expired_keys"`
	DisabledKeys      int                      `json:"disabled_keys"`
	ActiveSessions    int                      `json:"active_sessions"`
	TotalOperations   int                      `json:"total_operations"`
	SuccessfulOps     int                      `json:"successful_ops"`
	FailedOps         int                      `json:"failed_ops"`
	KeysByType        map[HSMKeyType]int       `json:"keys_by_type"`
	KeysByAccessLevel map[HSMKeyAccess]int    `json:"keys_by_access_level"`
	OperationsByType map[string]int           `json:"operations_by_type"`
}

// LogHSMEvent logs HSM events
func (sl *SecurityLogger) LogHSMEvent(eventType, description string, details map[string]interface{}) {
	event := SecurityEvent{
		Type:        SecurityEventType("hsm_event"),
		Severity:    SeverityMedium,
		Description: description,
		Details: map[string]interface{}{
			"hsm_event_type": eventType,
		},
	}
	
	if details != nil {
		for k, v := range details {
			event.Details[k] = v
		}
	}
	
	sl.LogEvent(event)
}

// LogHSMOperation logs HSM operations
func (sl *SecurityLogger) LogHSMOperation(operation *HSMOperation) {
	event := SecurityEvent{
		Type:        SecurityEventType("hsm_operation"),
		Severity:    SeverityInfo,
		Description: fmt.Sprintf("HSM operation: %s", operation.Type),
		Details: map[string]interface{}{
			"operation_type": operation.Type,
			"key_id":        operation.KeyID,
			"session_id":    operation.SessionID,
			"user_id":       operation.UserID,
			"success":       operation.Success,
			"duration":      operation.Duration,
		},
	}
	
	if !operation.Success {
		event.Severity = SeverityHigh
		event.Details["error"] = operation.Error
	}
	
	sl.LogEvent(event)
}