package security

import (
	
	"testing"
	"time"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// ========================================
// REAL SECURITY FOUNDATION TESTS
// ========================================

func TestNewSecurityManager(t *testing.T) {
	// Create valid security configuration
	config := createValidSecurityConfig()
	
	// Create security manager
	sm := NewSecurityManager(config)
	
	// Validate security manager creation
	assert.NotNil(t, sm, "Security manager should not be nil")
	assert.NotNil(t, sm.config, "Security config should not be nil")
	assert.False(t, sm.isInitialized, "Security manager should not be initialized initially")
}

func TestSecurityManagerInitializeSuccess(t *testing.T) {
	// Create valid security configuration
	config := createValidSecurityConfig()
	
	// Create and initialize security manager
	sm := NewSecurityManager(config)
	err := sm.InitializeSecurityManager()
	
	// Validate successful initialization
	assert.NoError(t, err, "Security manager initialization should succeed")
	assert.True(t, sm.isInitialized, "Security manager should be initialized")
}

func TestSecurityManagerInitializeInvalidConfig(t *testing.T) {
	// Test cases for invalid configuration
	testCases := []struct {
		name   string
		config *SecurityConfig
		errorMsg string
	}{
		{
			name:   "Empty JWT secret key",
			config: createSecurityConfigWithEmptyJWTSecret(),
			errorMsg: "JWT secret key is required",
		},
		{
			name:   "Short JWT secret key",
			config: createSecurityConfigWithShortJWTSecret(),
			errorMsg: "JWT secret key must be at least 32 characters",
		},
		{
			name:   "Empty password min length",
			config: createSecurityConfigWithInvalidPasswordMinLength(),
			errorMsg: "password minimum length must be positive",
		},
		{
			name:   "Invalid bcrypt cost",
			config: createSecurityConfigWithInvalidBCryptCost(),
			errorMsg: "bcrypt cost must be between 10 and 31",
		},
		{
			name:   "Empty Redis address",
			config: createSecurityConfigWithEmptyRedisAddr(),
			errorMsg: "Redis address is required",
		},
		{
			name:   "Invalid AES key length",
			config: createSecurityConfigWithInvalidAESKey(),
			errorMsg: "AES key must be exactly 32 characters",
		},
	}
	
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			// Create and initialize security manager with invalid config
			sm := NewSecurityManager(tc.config)
			err := sm.InitializeSecurityManager()
			
			// Validate initialization failure
			assert.Error(t, err, "Security manager initialization should fail")
			assert.Contains(t, err.Error(), tc.errorMsg, "Error message should contain expected text")
			assert.False(t, sm.isInitialized, "Security manager should not be initialized")
		})
	}
}

func TestSecurityManagerValidateInputSuccess(t *testing.T) {
	// Create security manager
	config := createValidSecurityConfig()
	sm := NewSecurityManager(config)
	err := sm.InitializeSecurityManager()
	require.NoError(t, err)
	
	// Test valid inputs
	testCases := []struct {
		name      string
		input     string
		inputType string
	}{
		{
			name:      "Valid username",
			input:     "john_doe_123",
			inputType: "username",
		},
		{
			name:      "Valid email",
			input:     "user@example.com",
			inputType: "email",
		},
		{
			name:      "Valid password",
			input:     "TestPassword123!",
			inputType: "password",
		},
	}
	
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			err := sm.ValidateInput(tc.input, tc.inputType)
			assert.NoError(t, err, "Input validation should succeed for valid input")
		})
	}
}

func TestSecurityManagerValidateInputFailure(t *testing.T) {
	// Create security manager
	config := createValidSecurityConfig()
	sm := NewSecurityManager(config)
	err := sm.InitializeSecurityManager()
	require.NoError(t, err)
	
	// Test invalid inputs
	testCases := []struct {
		name      string
		input     string
		inputType string
		errorMsg  string
	}{
		{
			name:      "Empty input",
			input:     "",
			inputType: "username",
			errorMsg:  "input cannot be empty",
		},
		{
			name:      "Too short username",
			input:     "jd",
			inputType: "username",
			errorMsg:  "username must be between 3 and 50 characters",
		},
		{
			name:      "Too long username",
			input:     string(make([]byte, 60)), // 60 characters
			inputType: "username",
			errorMsg:  "username must be between 3 and 50 characters",
		},
		{
			name:      "SQL injection attempt",
			input:     "admin'; DROP TABLE users; --",
			inputType: "username",
			errorMsg:  "input contains potentially dangerous characters",
		},
		{
			name:      "XSS attempt",
			input:     "<script>alert('xss')</script>",
			inputType: "username",
			errorMsg:  "input contains potentially dangerous HTML/JavaScript",
		},
	}
	
	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			err := sm.ValidateInput(tc.input, tc.inputType)
			assert.Error(t, err, "Input validation should fail for invalid input")
			assert.Contains(t, err.Error(), tc.errorMsg, "Error message should contain expected text")
		})
	}
}

func TestSecurityManagerGetSecurityStatus(t *testing.T) {
	// Create security manager
	config := createValidSecurityConfig()
	sm := NewSecurityManager(config)
	err := sm.InitializeSecurityManager()
	require.NoError(t, err)
	
	// Get security status
	status := sm.GetSecurityStatus()
	
	// Validate security status
	assert.NotNil(t, status, "Security status should not be nil")
	assert.True(t, status.Initialized, "Security status should show initialized")
	assert.True(t, status.JWTEnabled, "JWT should be enabled")
	assert.True(t, status.SessionEnabled, "Session should be enabled")
	assert.True(t, status.RateLimitEnabled, "Rate limiting should be enabled")
	assert.True(t, status.EncryptionEnabled, "Encryption should be enabled")
	assert.True(t, status.AuditLoggingEnabled, "Audit logging should be enabled")
}

func TestSecurityManagerGetSecurityMetrics(t *testing.T) {
	// Create security manager
	config := createValidSecurityConfig()
	sm := NewSecurityManager(config)
	err := sm.InitializeSecurityManager()
	require.NoError(t, err)
	
	// Get security metrics
	metrics := sm.GetSecurityMetrics()
	
	// Validate security metrics
	assert.NotNil(t, metrics, "Security metrics should not be nil")
	assert.True(t, metrics.TokensIssued > 0, "Tokens issued should be positive")
	assert.True(t, metrics.TokensValidated > 0, "Tokens validated should be positive")
	assert.True(t, metrics.SessionsCreated > 0, "Sessions created should be positive")
	assert.True(t, metrics.RateLimitRequests > 0, "Rate limit requests should be positive")
}

// ========================================
// UTILITY FUNCTIONS FOR TESTS
// ========================================

// createValidSecurityConfig creates a valid security configuration for testing
func createValidSecurityConfig() *SecurityConfig {
	return &SecurityConfig{
		JWT: struct {
			SecretKey     string `json:"secret_key" yaml:"secret_key"`
			Issuer        string `json:"issuer" yaml:"issuer"`
			ExpirationHrs int    `json:"expiration_hrs" yaml:"expiration_hrs"`
		}{
			SecretKey:     "test-secret-key-32-characters-long",
			Issuer:        "vaughan-cli-test",
			ExpirationHrs: 24,
		},
		Password: struct {
			MinLength int    `json:"min_length" yaml:"min_length"`
			MaxAge    int    `json:"max_age_days" yaml:"max_age_days"`
			BCryptCost int   `json:"bcrypt_cost" yaml:"bcrypt_cost"`
		}{
			MinLength: 8,
			MaxAge:    90,
			BCryptCost: 12,
		},
		Session: struct {
			RedisAddr     string        `json:"redis_addr" yaml:"redis_addr"`
			RedisPassword string        `json:"redis_password" yaml:"redis_password"`
			RedisDB       int           `json:"redis_db" yaml:"redis_db"`
			Expiration     time.Duration `json:"expiration" yaml:"expiration"`
		}{
			RedisAddr:     "localhost:6379",
			RedisPassword: "",
			RedisDB:       0,
			Expiration:     time.Hour,
		},
		RateLimit: struct {
			RequestsPerSecond float64 `json:"requests_per_second" yaml:"requests_per_second"`
			BurstSize        int     `json:"burst_size" yaml:"burst_size"`
		}{
			RequestsPerSecond: 10.0,
			BurstSize:        20,
		},
		Encryption: struct {
			AESKey string `json:"aes_key" yaml:"aes_key"`
		}{
			AESKey: "12345678901234567890123456789012", // 32 chars
		},
		Audit: struct {
			LogLevel  string `json:"log_level" yaml:"log_level"`
			LogFile   string `json:"log_file" yaml:"log_file"`
		}{
			LogLevel: "info",
			LogFile:  "/var/log/vaughan/security.log",
		},
	}
}

// Functions to create invalid security configurations for testing
func createSecurityConfigWithEmptyJWTSecret() *SecurityConfig {
	config := createValidSecurityConfig()
	config.JWT.SecretKey = ""
	return config
}

func createSecurityConfigWithShortJWTSecret() *SecurityConfig {
	config := createValidSecurityConfig()
	config.JWT.SecretKey = "short"
	return config
}

func createSecurityConfigWithInvalidPasswordMinLength() *SecurityConfig {
	config := createValidSecurityConfig()
	config.Password.MinLength = 0
	return config
}

func createSecurityConfigWithInvalidBCryptCost() *SecurityConfig {
	config := createValidSecurityConfig()
	config.Password.BCryptCost = 5
	return config
}

func createSecurityConfigWithEmptyRedisAddr() *SecurityConfig {
	config := createValidSecurityConfig()
	config.Session.RedisAddr = ""
	return config
}

func createSecurityConfigWithInvalidAESKey() *SecurityConfig {
	config := createValidSecurityConfig()
	config.Encryption.AESKey = "short"
	return config
}