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
	"strings"
	"time"
)

// KeyManager manages secure API key storage
type KeyManager struct {
	keyFile string
	gcm     cipher.AEAD
}

// NewKeyManager creates a new key manager
func NewKeyManager(keyFile string) (*KeyManager, error) {
	km := &KeyManager{
		keyFile: keyFile,
	}
	
	// Ensure directory exists
	dir := filepath.Dir(keyFile)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return nil, fmt.Errorf("failed to create key directory: %w", err)
	}
	
	// Initialize encryption key
	if err := km.initEncryption(); err != nil {
		return nil, fmt.Errorf("failed to initialize encryption: %w", err)
	}
	
	return km, nil
}

// initEncryption initializes AES encryption
func (km *KeyManager) initEncryption() error {
	// Derive key from system entropy or user password
	key := km.deriveKey()
	
	block, err := aes.NewCipher(key)
	if err != nil {
		return fmt.Errorf("failed to create cipher: %w", err)
	}
	
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return fmt.Errorf("failed to create GCM: %w", err)
	}
	
	km.gcm = gcm
	return nil
}

// deriveKey derives encryption key from system
func (km *KeyManager) deriveKey() []byte {
	// Get system-specific entropy
	hostname, _ := os.Hostname()
	user := os.Getenv("USER")
	home := os.Getenv("HOME")
	
	// Create salt
	salt := hostname + user + home
	
	// Derive key using SHA-256
	hash := sha256.Sum256([]byte(salt))
	return hash[:]
}

// StoreKey securely stores an API key
func (km *KeyManager) StoreKey(service, key string) error {
	// Encrypt the key
	nonce := make([]byte, km.gcm.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return fmt.Errorf("failed to generate nonce: %w", err)
	}
	
	encrypted := km.gcm.Seal(nonce, nonce, []byte(key), nil)
	
	// Store in key file
	keyData := map[string]string{
		"service":  service,
		"key":      base64.StdEncoding.EncodeToString(encrypted),
		"created":  time.Now().Format(time.RFC3339),
	}
	
	return km.appendKeyData(keyData)
}

// GetKey retrieves an API key
func (km *KeyManager) GetKey(service string) (string, error) {
	keyData, err := km.readKeyData()
	if err != nil {
		return "", err
	}
	
	for _, entry := range keyData {
		if entry["service"] == service {
			encrypted, err := base64.StdEncoding.DecodeString(entry["key"])
			if err != nil {
				return "", fmt.Errorf("failed to decode key: %w", err)
			}
			
			nonceSize := km.gcm.NonceSize()
			if len(encrypted) < nonceSize {
				return "", fmt.Errorf("invalid encrypted data")
			}
			
			nonce := encrypted[:nonceSize]
			ciphertext := encrypted[nonceSize:]
			
			decrypted, err := km.gcm.Open(nil, nonce, ciphertext, nil)
			if err != nil {
				return "", fmt.Errorf("failed to decrypt key: %w", err)
			}
			
			return string(decrypted), nil
		}
	}
	
	return "", fmt.Errorf("key not found for service: %s", service)
}

// DeleteKey removes an API key
func (km *KeyManager) DeleteKey(service string) error {
	keyData, err := km.readKeyData()
	if err != nil {
		return err
	}
	
	// Filter out the key to delete
	var newData []map[string]string
	for _, entry := range keyData {
		if entry["service"] != service {
			newData = append(newData, entry)
		}
	}
	
	return km.writeKeyData(newData)
}

// RotateKey rotates an API key
func (km *KeyManager) RotateKey(service, newKey string) error {
	if err := km.DeleteKey(service); err != nil {
		return fmt.Errorf("failed to delete old key: %w", err)
	}
	
	return km.StoreKey(service, newKey)
}

// appendKeyData appends key data to storage
func (km *KeyManager) appendKeyData(keyData map[string]string) error {
	// Load existing data
	data, err := km.readKeyData()
	if err != nil {
		data = []map[string]string{}
	}
	
	// Append new data
	data = append(data, keyData)
	
	// Write back
	return km.writeKeyData(data)
}

// readKeyData reads key data from storage
func (km *KeyManager) readKeyData() ([]map[string]string, error) {
	if _, err := os.Stat(km.keyFile); os.IsNotExist(err) {
		return []map[string]string{}, nil
	}
	
	data, err := os.ReadFile(km.keyFile)
	if err != nil {
		return nil, fmt.Errorf("failed to read key file: %w", err)
	}
	
	// Parse key data
	var keyData []map[string]string
	if err := json.Unmarshal(data, &keyData); err != nil {
		return nil, fmt.Errorf("failed to parse key data: %w", err)
	}
	
	return keyData, nil
}

// writeKeyData writes key data to storage
func (km *KeyManager) writeKeyData(data []map[string]string) error {
	jsonData, err := json.MarshalIndent(data, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal key data: %w", err)
	}
	
	// Write with secure permissions
	return os.WriteFile(km.keyFile, jsonData, 0600)
}

// ValidateKey checks if key meets security requirements
func ValidateKey(service, key string) error {
	// Check key length
	if len(key) < 32 {
		return fmt.Errorf("key too short for service %s", service)
	}
	
	// Check for common weak keys
	weakKeys := []string{
		"password", "123456", "admin", "test", "default",
	}
	
	for _, weak := range weakKeys {
		if strings.Contains(strings.ToLower(key), weak) {
			return fmt.Errorf("key contains weak pattern for service %s", service)
		}
	}
	
	return nil
}

// KeyRotationManager manages automatic key rotation
type KeyRotationManager struct {
	keyManager     *KeyManager
	rotationRules map[string]time.Duration
}

// NewKeyRotationManager creates a key rotation manager
func NewKeyRotationManager(keyManager *KeyManager) *KeyRotationManager {
	return &KeyRotationManager{
		keyManager:     keyManager,
		rotationRules: make(map[string]time.Duration),
	}
}

// SetRotationRule sets rotation rule for a service
func (krm *KeyRotationManager) SetRotationRule(service string, interval time.Duration) {
	krm.rotationRules[service] = interval
}

// CheckRotation checks if keys need rotation
func (krm *KeyRotationManager) CheckRotation() []string {
	var needsRotation []string
	
	keyData, _ := krm.keyManager.readKeyData()
	for _, entry := range keyData {
		service := entry["service"]
		createdStr := entry["created"]
		
		if interval, exists := krm.rotationRules[service]; exists {
			created, _ := time.Parse(time.RFC3339, createdStr)
			if time.Since(created) > interval {
				needsRotation = append(needsRotation, service)
			}
		}
	}
	
	return needsRotation
}
