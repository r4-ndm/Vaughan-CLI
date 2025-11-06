package security

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"
)

// ConfigSecurity manages secure configuration
type ConfigSecurity struct {
	configPath    string
	encryptionKey []byte
	mutex         sync.RWMutex
	logger        *SecurityLogger
	backupDir     string
}

// SecureConfig represents encrypted configuration
type SecureConfig struct {
	Version     string                 `json:"version"`
	Environment string                 `json:"environment"`
	Values      map[string]interface{} `json:"values"`
	Secrets    map[string]string     `json:"secrets"`
	CreatedAt   time.Time              `json:"created_at"`
	UpdatedAt   time.Time              `json:"updated_at"`
	Hash        string                 `json:"hash"`
	Signature   string                 `json:"signature"`
}

// ConfigHistory represents configuration change history
type ConfigHistory struct {
	ID        string                 `json:"id"`
	Version   string                 `json:"version"`
	Changes   map[string]interface{} `json:"changes"`
	UserID    string                 `json:"user_id"`
	SessionID string                 `json:"session_id"`
	Timestamp time.Time              `json:"timestamp"`
	Reason    string                 `json:"reason"`
	Hash      string                 `json:"hash"`
}

// ConfigRule represents configuration validation rules
type ConfigRule struct {
	Name        string              `json:"name"`
	Type        string              `json:"type"` // string, number, boolean, array, object
	Required    bool                `json:"required"`
	Min         interface{}         `json:"min,omitempty"`
	Max         interface{}         `json:"max,omitempty"`
	Pattern     string              `json:"pattern,omitempty"`
	Allowed     []interface{}       `json:"allowed,omitempty"`
	Blocked     []interface{}       `json:"blocked,omitempty"`
	Secret      bool                `json:"secret"` // indicates sensitive config value
	Encrypt     bool                `json:"encrypt"` // indicates value should be encrypted
	Description string              `json:"description"`
}

// NewConfigSecurity creates secure configuration manager
func NewConfigSecurity(configPath string, logger *SecurityLogger) (*ConfigSecurity, error) {
	cs := &ConfigSecurity{
		configPath: configPath,
		logger:     logger,
		backupDir:  configPath + ".backups",
	}
	
	// Create backup directory
	if err := os.MkdirAll(cs.backupDir, 0700); err != nil {
		return nil, fmt.Errorf("failed to create backup directory: %w", err)
	}
	
	// Initialize encryption key
	if err := cs.initEncryptionKey(); err != nil {
		return nil, fmt.Errorf("failed to initialize encryption: %w", err)
	}
	
	return cs, nil
}

// LoadConfig loads and decrypts configuration
func (cs *ConfigSecurity) LoadConfig() (*SecureConfig, error) {
	cs.mutex.RLock()
	defer cs.mutex.RUnlock()
	
	// Check if config file exists
	if _, err := os.Stat(cs.configPath); os.IsNotExist(err) {
		// Return default config if file doesn't exist
		return cs.createDefaultConfig()
	}
	
	// Read encrypted config
	data, err := os.ReadFile(cs.configPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read config file: %w", err)
	}
	
	// Decrypt config
	decrypted, err := cs.decryptData(data)
	if err != nil {
		return nil, fmt.Errorf("failed to decrypt config: %w", err)
	}
	
	// Parse config
	var config SecureConfig
	if err := json.Unmarshal(decrypted, &config); err != nil {
		return nil, fmt.Errorf("failed to parse config: %w", err)
	}
	
	// Verify config integrity
	if err := cs.verifyConfigIntegrity(&config); err != nil {
		return nil, fmt.Errorf("config integrity check failed: %w", err)
	}
	
	return &config, nil
}

// SaveConfig encrypts and saves configuration
func (cs *ConfigSecurity) SaveConfig(config *SecureConfig, reason string, ctx *Context) error {
	cs.mutex.Lock()
	defer cs.mutex.Unlock()
	
	// Create backup before saving
	if err := cs.createBackup(reason, ctx); err != nil {
		cs.logger.LogConfigChange(ctx.UserID, ctx.SessionID, "backup_failed", map[string]interface{}{
			"error": err.Error(),
		})
		return fmt.Errorf("failed to create backup: %w", err)
	}
	
	// Update config metadata
	config.UpdatedAt = time.Now()
	config.Hash = cs.calculateConfigHash(config)
	config.Signature = cs.signConfig(config)
	
	// Validate config
	if err := cs.validateConfig(config); err != nil {
		return fmt.Errorf("config validation failed: %w", err)
	}
	
	// Serialize config
	data, err := json.MarshalIndent(config, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to serialize config: %w", err)
	}
	
	// Encrypt config
	encrypted, err := cs.encryptData(data)
	if err != nil {
		return fmt.Errorf("failed to encrypt config: %w", err)
	}
	
	// Save encrypted config
	if err := os.WriteFile(cs.configPath, encrypted, 0600); err != nil {
		return fmt.Errorf("failed to save config: %w", err)
	}
	
	// Log config change
	if cs.logger != nil {
		cs.logger.LogConfigChange(ctx.UserID, ctx.SessionID, "config_saved", map[string]interface{}{
			"reason":  reason,
			"version": config.Version,
			"hash":    config.Hash,
		})
	}
	
	return nil
}

// UpdateConfig updates specific configuration values
func (cs *ConfigSecurity) UpdateConfig(updates map[string]interface{}, rules map[string]ConfigRule, reason string, ctx *Context) error {
	// Load current config
	config, err := cs.LoadConfig()
	if err != nil {
		return fmt.Errorf("failed to load current config: %w", err)
	}
	
	// Track changes for history
	changes := make(map[string]interface{})
	
	// Apply updates with validation
	for key, value := range updates {
		rule, exists := rules[key]
		if !exists {
			return fmt.Errorf("no validation rule for config key: %s", key)
		}
		
		// Validate value
		if err := cs.validateConfigValue(key, value, rule); err != nil {
			return fmt.Errorf("validation failed for %s: %w", key, err)
		}
		
		// Store change
		oldValue := config.Values[key]
		changes[key] = map[string]interface{}{
			"old": oldValue,
			"new": value,
		}
		
		// Apply update
		if rule.Secret || rule.Encrypt {
			if config.Secrets == nil {
				config.Secrets = make(map[string]string)
			}
			config.Secrets[key] = fmt.Sprintf("%v", value)
			if config.Values != nil {
				delete(config.Values, key)
			}
		} else {
			if config.Values == nil {
				config.Values = make(map[string]interface{})
			}
			config.Values[key] = value
			if config.Secrets != nil {
				delete(config.Secrets, key)
			}
		}
	}
	
	// Save updated config
	if err := cs.SaveConfig(config, reason, ctx); err != nil {
		return err
	}
	
	// Record config change history
	if err := cs.recordConfigChange(config.Version, changes, reason, ctx); err != nil {
		// Log warning but don't fail
		if cs.logger != nil {
			cs.logger.LogConfigChange(ctx.UserID, ctx.SessionID, "history_failed", map[string]interface{}{
				"error": err.Error(),
			})
		}
	}
	
	return nil
}

// GetConfigValue retrieves specific configuration value
func (cs *ConfigSecurity) GetConfigValue(key string, rules map[string]ConfigRule) (interface{}, error) {
	config, err := cs.LoadConfig()
	if err != nil {
		return nil, err
	}
	
	// Check if it's a secret value
	if secretValue, exists := config.Secrets[key]; exists {
		return secretValue, nil
	}
	
	// Check regular value
	if value, exists := config.Values[key]; exists {
		// Apply rule-based formatting if needed
		if rule, exists := rules[key]; exists && rule.Secret {
			return cs.maskSecretValue(fmt.Sprintf("%v", value)), nil
		}
		return value, nil
	}
	
	// Check if key is required
	if rule, exists := rules[key]; exists && rule.Required {
		return nil, fmt.Errorf("required config value not found: %s", key)
	}
	
	return nil, nil
}

// validateConfig validates entire configuration
func (cs *ConfigSecurity) validateConfig(config *SecureConfig) error {
	// Check required fields
	if config.Version == "" {
		return fmt.Errorf("config version is required")
	}
	
	if config.Values == nil && config.Secrets == nil {
		return fmt.Errorf("config must have values or secrets")
	}
	
	// Check for duplicate keys between values and secrets
	for key := range config.Values {
		if _, exists := config.Secrets[key]; exists {
			return fmt.Errorf("duplicate config key: %s", key)
		}
	}
	
	return nil
}

// validateConfigValue validates individual configuration value
func (cs *ConfigSecurity) validateConfigValue(key string, value interface{}, rule ConfigRule) error {
	// Check required
	if rule.Required && (value == nil || value == "") {
		return fmt.Errorf("required value is missing or empty")
	}
	
	if value == nil {
		return nil // Optional value is missing
	}
	
	// Type validation
	switch rule.Type {
	case "string":
		if _, ok := value.(string); !ok {
			return fmt.Errorf("value must be a string")
		}
		
		strValue := value.(string)
		
		// Pattern validation
		if rule.Pattern != "" {
			matched, err := regexp.MatchString(rule.Pattern, strValue)
			if err != nil {
				return fmt.Errorf("invalid regex pattern: %w", err)
			}
			if !matched {
				return fmt.Errorf("value does not match required pattern")
			}
		}
		
		// Length validation
		if rule.Min != nil {
			if minLen, ok := rule.Min.(int); ok && len(strValue) < minLen {
				return fmt.Errorf("value too short: minimum %d characters", minLen)
			}
		}
		
		if rule.Max != nil {
			if maxLen, ok := rule.Max.(int); ok && len(strValue) > maxLen {
				return fmt.Errorf("value too long: maximum %d characters", maxLen)
			}
		}
		
	case "number":
		var numValue float64
		switch v := value.(type) {
		case int:
			numValue = float64(v)
		case float64:
			numValue = v
		case string:
			parsed, err := strconv.ParseFloat(v, 64)
			if err != nil {
				return fmt.Errorf("value must be a number")
			}
			numValue = parsed
		default:
			return fmt.Errorf("value must be a number")
		}
		
		// Range validation
		if rule.Min != nil {
			if minVal, ok := rule.Min.(float64); ok && numValue < minVal {
				return fmt.Errorf("value too small: minimum %f", minVal)
			}
		}
		
		if rule.Max != nil {
			if maxVal, ok := rule.Max.(float64); ok && numValue > maxVal {
				return fmt.Errorf("value too large: maximum %f", maxVal)
			}
		}
		
	case "boolean":
		if _, ok := value.(bool); !ok {
			return fmt.Errorf("value must be a boolean")
		}
		
	case "array":
		if _, ok := value.([]interface{}); !ok {
			return fmt.Errorf("value must be an array")
		}
		
		// Length validation
		array := value.([]interface{})
		if rule.Min != nil {
			if minLen, ok := rule.Min.(int); ok && len(array) < minLen {
				return fmt.Errorf("array too short: minimum %d items", minLen)
			}
		}
		
		if rule.Max != nil {
			if maxLen, ok := rule.Max.(int); ok && len(array) > maxLen {
				return fmt.Errorf("array too long: maximum %d items", maxLen)
			}
		}
		
	case "object":
		if _, ok := value.(map[string]interface{}); !ok {
			return fmt.Errorf("value must be an object")
		}
	}
	
	// Allowed values validation
	if len(rule.Allowed) > 0 {
		allowed := false
		for _, allowedValue := range rule.Allowed {
			if value == allowedValue {
				allowed = true
				break
			}
		}
		if !allowed {
			return fmt.Errorf("value not in allowed list")
		}
	}
	
	// Blocked values validation
	for _, blockedValue := range rule.Blocked {
		if value == blockedValue {
			return fmt.Errorf("value is blocked")
		}
	}
	
	return nil
}

// initEncryptionKey initializes encryption key for configuration
func (cs *ConfigSecurity) initEncryptionKey() error {
	// Derive key from system-specific entropy
	hostname, _ := os.Hostname()
	user := os.Getenv("USER")
	home := os.Getenv("HOME")
	
	// Create salt
	salt := fmt.Sprintf("vaughan-crush-config-%s-%s-%s", hostname, user, home)
	
	// Derive key using SHA-256
	hash := sha256.Sum256([]byte(salt))
	cs.encryptionKey = hash[:]
	
	return nil
}

// encryptData encrypts data with AES-GCM
func (cs *ConfigSecurity) encryptData(data []byte) ([]byte, error) {
	block, err := aes.NewCipher(cs.encryptionKey)
	if err != nil {
		return nil, err
	}
	
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	
	nonce := make([]byte, gcm.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return nil, err
	}
	
	return gcm.Seal(nonce, nonce, data, nil), nil
}

// decryptData decrypts data with AES-GCM
func (cs *ConfigSecurity) decryptData(data []byte) ([]byte, error) {
	block, err := aes.NewCipher(cs.encryptionKey)
	if err != nil {
		return nil, err
	}
	
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	
	nonceSize := gcm.NonceSize()
	if len(data) < nonceSize {
		return nil, fmt.Errorf("ciphertext too short")
	}
	
	nonce, ciphertext := data[:nonceSize], data[nonceSize:]
	return gcm.Open(nil, nonce, ciphertext, nil)
}

// createDefaultConfig creates default configuration
func (cs *ConfigSecurity) createDefaultConfig() (*SecureConfig, error) {
	return &SecureConfig{
		Version:     "1.0.0",
		Environment: "development",
		Values:      make(map[string]interface{}),
		Secrets:    make(map[string]string),
		CreatedAt:   time.Now(),
		UpdatedAt:   time.Now(),
	}, nil
}

// createBackup creates configuration backup
func (cs *ConfigSecurity) createBackup(reason string, ctx *Context) error {
	// Check if current config exists
	if _, err := os.Stat(cs.configPath); os.IsNotExist(err) {
		return nil // No backup needed for new config
	}
	
	// Create backup filename
	backupPath := filepath.Join(cs.backupDir, fmt.Sprintf("config_%s_%s.enc", 
		time.Now().Format("20060102_150405"), 
		ctx.UserID))
	
	// Copy current config to backup
	data, err := os.ReadFile(cs.configPath)
	if err != nil {
		return err
	}
	
	return os.WriteFile(backupPath, data, 0600)
}

// calculateConfigHash calculates configuration hash for integrity
func (cs *ConfigSecurity) calculateConfigHash(config *SecureConfig) string {
	// Create hash data
	hashData := fmt.Sprintf("%s|%s|%v|%v|%s|%s",
		config.Version,
		config.Environment,
		config.Values,
		config.Secrets,
		config.CreatedAt.String(),
		config.UpdatedAt.String())
	
	hash := sha256.Sum256([]byte(hashData))
	return base64.StdEncoding.EncodeToString(hash[:])
}

// signConfig creates digital signature for configuration
func (cs *ConfigSecurity) signConfig(config *SecureConfig) string {
	// Simplified signature
	// In production, use proper digital signatures
	return fmt.Sprintf("signed_%d", time.Now().Unix())
}

// verifyConfigIntegrity verifies configuration integrity
func (cs *ConfigSecurity) verifyConfigIntegrity(config *SecureConfig) error {
	// Recalculate hash
	expectedHash := cs.calculateConfigHash(config)
	
	if config.Hash != expectedHash {
		return fmt.Errorf("config hash mismatch: possible tampering")
	}
	
	// Verify signature (simplified)
	if config.Signature == "" {
		return fmt.Errorf("missing config signature")
	}
	
	return nil
}

// maskSecretValue masks sensitive configuration values
func (cs *ConfigSecurity) maskSecretValue(value string) string {
	if len(value) <= 4 {
		return strings.Repeat("*", len(value))
	}
	
	return value[:2] + strings.Repeat("*", len(value)-4) + value[len(value)-2:]
}

// recordConfigChange records configuration change history
func (cs *ConfigSecurity) recordConfigChange(version string, changes map[string]interface{}, reason string, ctx *Context) error {
	history := &ConfigHistory{
		ID:        fmt.Sprintf("change_%d", time.Now().UnixNano()),
		Version:   version,
		Changes:   changes,
		UserID:    ctx.UserID,
		SessionID: ctx.SessionID,
		Timestamp: time.Now(),
		Reason:    reason,
		Hash:      cs.calculateChangeHash(changes),
	}
	
	// Serialize history
	data, err := json.MarshalIndent(history, "", "  ")
	if err != nil {
		return err
	}
	
	// Save to history file
	historyPath := filepath.Join(cs.backupDir, "config_history.jsonl")
	file, err := os.OpenFile(historyPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
	if err != nil {
		return err
	}
	defer file.Close()
	
	_, err = file.Write(append(data, '\n'))
	return err
}

// calculateChangeHash calculates hash for changes
func (cs *ConfigSecurity) calculateChangeHash(changes map[string]interface{}) string {
	data, _ := json.Marshal(changes)
	hash := sha256.Sum256(data)
	return base64.StdEncoding.EncodeToString(hash[:])
}

// LogConfigChange logs configuration changes
func (sl *SecurityLogger) LogConfigChange(userID, sessionID, action string, details map[string]interface{}) {
	event := SecurityEvent{
		Type:        "config_change",
		Severity:    SeverityMedium,
		UserID:      userID,
		SessionID:   sessionID,
		Description: fmt.Sprintf("Configuration %s", action),
		Details:     details,
	}
	
	if action == "config_saved" {
		event.Severity = SeverityInfo
	} else if action == "backup_failed" || action == "history_failed" {
		event.Severity = SeverityHigh
	}
	
	sl.LogEvent(event)
}

// GetConfigHistory returns configuration change history
func (cs *ConfigSecurity) GetConfigHistory(limit int) ([]*ConfigHistory, error) {
	historyPath := filepath.Join(cs.backupDir, "config_history.jsonl")
	
	data, err := os.ReadFile(historyPath)
	if err != nil {
		if os.IsNotExist(err) {
			return []*ConfigHistory{}, nil
		}
		return nil, err
	}
	
	lines := strings.Split(string(data), "\n")
	history := make([]*ConfigHistory, 0)
	
	for _, line := range lines {
		if strings.TrimSpace(line) == "" {
			continue
		}
		
		var entry ConfigHistory
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			continue // Skip invalid entries
		}
		
		history = append(history, &entry)
	}
	
	// Return most recent entries
	if limit > 0 && len(history) > limit {
		history = history[len(history)-limit:]
	}
	
	return history, nil
}

// GetBackups returns list of configuration backups
func (cs *ConfigSecurity) GetBackups() ([]string, error) {
	files, err := os.ReadDir(cs.backupDir)
	if err != nil {
		return nil, err
	}
	
	var backups []string
	for _, file := range files {
		if !file.IsDir() && strings.HasSuffix(file.Name(), ".enc") {
			backups = append(backups, file.Name())
		}
	}
	
	return backups, nil
}

// RestoreBackup restores configuration from backup
func (cs *ConfigSecurity) RestoreBackup(backupName string, ctx *Context) error {
	backupPath := filepath.Join(cs.backupDir, backupName)
	
	// Verify backup exists
	if _, err := os.Stat(backupPath); os.IsNotExist(err) {
		return fmt.Errorf("backup not found: %s", backupName)
	}
	
	// Read backup
	data, err := os.ReadFile(backupPath)
	if err != nil {
		return fmt.Errorf("failed to read backup: %w", err)
	}
	
	// Verify current config exists and create backup
	if _, err := os.Stat(cs.configPath); err == nil {
		if err := cs.createBackup("pre-restore", ctx); err != nil {
			return fmt.Errorf("failed to create pre-restore backup: %w", err)
		}
	}
	
	// Restore backup
	if err := os.WriteFile(cs.configPath, data, 0600); err != nil {
		return fmt.Errorf("failed to restore backup: %w", err)
	}
	
	// Log restoration
	if cs.logger != nil {
		cs.logger.LogConfigChange(ctx.UserID, ctx.SessionID, "config_restored", map[string]interface{}{
			"backup": backupName,
		})
	}
	
	return nil
}

// CleanupOldBackups removes old configuration backups
func (cs *ConfigSecurity) CleanupOldBackups(olderThan time.Duration) int {
	files, err := os.ReadDir(cs.backupDir)
	if err != nil {
		return 0
	}
	
	cutoff := time.Now().Add(-olderThan)
	removed := 0
	
	for _, file := range files {
		info, err := file.Info()
		if err != nil || info.ModTime().Before(cutoff) {
			filePath := filepath.Join(cs.backupDir, file.Name())
			if os.Remove(filePath) == nil {
				removed++
			}
		}
	}
	
	return removed
}