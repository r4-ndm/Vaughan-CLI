package security

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"fmt"
	"time"
)

// AuthenticationMethod represents different authentication methods
type AuthenticationMethod string

const (
	AuthMethodPassword AuthenticationMethod = "password"
	AuthMethodJWT     AuthenticationMethod = "jwt"
	AuthMethodAPIKey  AuthenticationMethod = "api_key"
	AuthMethodMFA     AuthenticationMethod = "mfa"
)

// AuthenticationResult represents the result of an authentication attempt
type AuthenticationResult struct {
	Success     bool                   `json:"success"`
	UserID      string                 `json:"user_id,omitempty"`
	SessionID   string                 `json:"session_id,omitempty"`
	Method      AuthenticationMethod    `json:"method"`
	Timestamp   time.Time              `json:"timestamp"`
	ExpiresAt   *time.Time             `json:"expires_at,omitempty"`
	Permissions []Permission           `json:"permissions,omitempty"`
	Error       string                 `json:"error,omitempty"`
	Metadata    map[string]interface{} `json:"metadata,omitempty"`
}

// User represents a system user
type User struct {
	ID           string                 `json:"id"`
	Username     string                 `json:"username"`
	Email        string                 `json:"email"`
	PasswordHash string                 `json:"password_hash"`
	Permissions  []Permission           `json:"permissions"`
	AuthMethods  []AuthenticationMethod `json:"auth_methods"`
	MFAEnabled   bool                   `json:"mfa_enabled"`
	MFASecret    string                 `json:"mfa_secret,omitempty"`
	CreatedAt    time.Time              `json:"created_at"`
	LastLogin    *time.Time             `json:"last_login,omitempty"`
	Active       bool                   `json:"active"`
	Metadata     map[string]interface{} `json:"metadata,omitempty"`
}

// Session represents an authenticated user session
type Session struct {
	ID           string                 `json:"id"`
	UserID       string                 `json:"user_id"`
	Token        string                 `json:"token"`
	Method       AuthenticationMethod    `json:"method"`
	CreatedAt    time.Time              `json:"created_at"`
	ExpiresAt    time.Time              `json:"expires_at"`
	LastActivity time.Time              `json:"last_activity"`
	Permissions  []Permission           `json:"permissions"`
	Metadata     map[string]interface{} `json:"metadata,omitempty"`
}

// Authenticator provides authentication functionality
type Authenticator struct {
	users      map[string]*User
	sessions   map[string]*Session
	logger     *SecurityLogger
	maxSession time.Duration
}

// NewAuthenticator creates a new authenticator
func NewAuthenticator(logger *SecurityLogger) *Authenticator {
	return &Authenticator{
		users:      make(map[string]*User),
		sessions:   make(map[string]*Session),
		logger:     logger,
		maxSession: 24 * time.Hour, // Default 24 hours
	}
}

// SetMaxSessionDuration sets the maximum session duration
func (a *Authenticator) SetMaxSessionDuration(duration time.Duration) {
	a.maxSession = duration
}

// CreateUser creates a new user
func (a *Authenticator) CreateUser(username, email, password string, permissions []Permission) (*User, error) {
	// Check if user already exists
	if _, exists := a.users[username]; exists {
		return nil, fmt.Errorf("user already exists: %s", username)
	}
	
	// Hash password
	hash, err := a.hashPassword(password)
	if err != nil {
		return nil, fmt.Errorf("failed to hash password: %w", err)
	}
	
	// Create user
	user := &User{
		ID:           a.generateUserID(),
		Username:     username,
		Email:        email,
		PasswordHash: hash,
		Permissions:  permissions,
		AuthMethods:  []AuthenticationMethod{AuthMethodPassword},
		MFAEnabled:   false,
		CreatedAt:    time.Now(),
		Active:       true,
		Metadata:     make(map[string]interface{}),
	}
	
	a.users[username] = user
	
	// Log user creation
	if a.logger != nil {
		a.logger.LogAuthEvent(user.ID, "", true, map[string]interface{}{
			"action":   "user_created",
			"username": username,
			"email":    email,
		})
	}
	
	return user, nil
}

// AuthenticatePassword authenticates user with password
func (a *Authenticator) AuthenticatePassword(username, password string) *AuthenticationResult {
	result := &AuthenticationResult{
		Method:    AuthMethodPassword,
		Timestamp: time.Now(),
	}
	
	// Get user
	user, exists := a.users[username]
	if !exists {
		result.Error = "user not found"
		if a.logger != nil {
			a.logger.LogAuthEvent("", "", false, map[string]interface{}{
				"username": username,
				"reason":   "user_not_found",
			})
		}
		return result
	}
	
	// Check if user is active
	if !user.Active {
		result.Error = "user account disabled"
		if a.logger != nil {
			a.logger.LogAuthEvent(user.ID, "", false, map[string]interface{}{
				"username": username,
				"reason":   "account_disabled",
			})
		}
		return result
	}
	
	// Verify password
	if !a.verifyPassword(password, user.PasswordHash) {
		result.Error = "invalid password"
		if a.logger != nil {
			a.logger.LogAuthEvent(user.ID, "", false, map[string]interface{}{
				"username": username,
				"reason":   "invalid_password",
			})
		}
		return result
	}
	
	// Authentication successful
	result.Success = true
	result.UserID = user.ID
	result.Permissions = user.Permissions
	
	// Create session
	session, err := a.createSession(user, AuthMethodPassword)
	if err != nil {
		result.Error = fmt.Sprintf("failed to create session: %v", err)
		return result
	}
	
	result.SessionID = session.ID
	result.ExpiresAt = &session.ExpiresAt
	
	// Update user last login
	now := time.Now()
	user.LastLogin = &now
	
	// Log successful authentication
	if a.logger != nil {
		a.logger.LogAuthEvent(user.ID, session.ID, true, map[string]interface{}{
			"method":   "password",
			"username": username,
		})
	}
	
	return result
}

// AuthenticateJWT authenticates user with JWT token
func (a *Authenticator) AuthenticateJWT(token string) *AuthenticationResult {
	result := &AuthenticationResult{
		Method:    AuthMethodJWT,
		Timestamp: time.Now(),
	}
	
	// Find session by token
	session, exists := a.sessions[token]
	if !exists {
		result.Error = "invalid token"
		if a.logger != nil {
			a.logger.LogAuthEvent("", "", false, map[string]interface{}{
				"reason": "invalid_token",
			})
		}
		return result
	}
	
	// Check if session is expired
	if time.Now().After(session.ExpiresAt) {
		delete(a.sessions, token)
		result.Error = "session expired"
		if a.logger != nil {
			a.logger.LogAuthEvent(session.UserID, session.ID, false, map[string]interface{}{
				"reason": "session_expired",
			})
		}
		return result
	}
	
	// Check if user is still active
	user, exists := a.users[session.UserID]
	if !exists || !user.Active {
		delete(a.sessions, token)
		result.Error = "user account disabled"
		if a.logger != nil {
			a.logger.LogAuthEvent(session.UserID, session.ID, false, map[string]interface{}{
				"reason": "account_disabled",
			})
		}
		return result
	}
	
	// Update session activity
	session.LastActivity = time.Now()
	
	// Authentication successful
	result.Success = true
	result.UserID = user.ID
	result.SessionID = session.ID
	result.Permissions = session.Permissions
	result.ExpiresAt = &session.ExpiresAt
	
	// Log successful authentication
	if a.logger != nil {
		a.logger.LogAuthEvent(user.ID, session.ID, true, map[string]interface{}{
			"method": "jwt",
		})
	}
	
	return result
}

// ValidateSession validates an existing session
func (a *Authenticator) ValidateSession(sessionID string) *AuthenticationResult {
	// Find session
	session, exists := a.sessions[sessionID]
	if !exists {
		return &AuthenticationResult{
			Success: false,
			Error:   "session not found",
		}
	}
	
	// Check if session is expired
	if time.Now().After(session.ExpiresAt) {
		delete(a.sessions, sessionID)
		return &AuthenticationResult{
			Success: false,
			Error:   "session expired",
		}
	}
	
	// Check if user is still active
	user, exists := a.users[session.UserID]
	if !exists || !user.Active {
		delete(a.sessions, sessionID)
		return &AuthenticationResult{
			Success: false,
			Error:   "user account disabled",
		}
	}
	
	// Update session activity
	session.LastActivity = time.Now()
	
	return &AuthenticationResult{
		Success:     true,
		UserID:      user.ID,
		SessionID:   sessionID,
		Method:      session.Method,
		Permissions: session.Permissions,
		Timestamp:   time.Now(),
		ExpiresAt:   &session.ExpiresAt,
	}
}

// Logout invalidates a session
func (a *Authenticator) Logout(sessionID string) error {
	session, exists := a.sessions[sessionID]
	if !exists {
		return fmt.Errorf("session not found")
	}
	
	delete(a.sessions, sessionID)
	
	// Log logout
	if a.logger != nil {
		a.logger.LogAuthEvent(session.UserID, sessionID, true, map[string]interface{}{
			"action": "logout",
		})
	}
	
	return nil
}

// CleanupExpiredSessions removes expired sessions
func (a *Authenticator) CleanupExpiredSessions() int {
	expired := 0
	now := time.Now()
	
	for token, session := range a.sessions {
		if now.After(session.ExpiresAt) {
			delete(a.sessions, token)
			expired++
		}
	}
	
	return expired
}

// GetActiveSessions returns active sessions for a user
func (a *Authenticator) GetActiveSessions(userID string) []*Session {
	var sessions []*Session
	
	for _, session := range a.sessions {
		if session.UserID == userID && time.Now().Before(session.ExpiresAt) {
			sessions = append(sessions, session)
		}
	}
	
	return sessions
}

// GetUser returns a user by ID
func (a *Authenticator) GetUser(userID string) (*User, bool) {
	for _, user := range a.users {
		if user.ID == userID {
			return user, true
		}
	}
	return nil, false
}

// GetUserByUsername returns a user by username
func (a *Authenticator) GetUserByUsername(username string) (*User, bool) {
	user, exists := a.users[username]
	return user, exists
}

// generateUserID generates a unique user ID
func (a *Authenticator) generateUserID() string {
	bytes := make([]byte, 16)
	rand.Read(bytes)
	return base64.URLEncoding.EncodeToString(bytes)
}

// createSession creates a new session for user
func (a *Authenticator) createSession(user *User, method AuthenticationMethod) (*Session, error) {
	// Generate session token
	bytes := make([]byte, 32)
	_, err := rand.Read(bytes)
	if err != nil {
		return nil, err
	}
	
	token := base64.URLEncoding.EncodeToString(bytes)
	
	session := &Session{
		ID:           a.generateSessionID(),
		UserID:       user.ID,
		Token:        token,
		Method:       method,
		CreatedAt:    time.Now(),
		ExpiresAt:    time.Now().Add(a.maxSession),
		LastActivity: time.Now(),
		Permissions:  user.Permissions,
		Metadata:     make(map[string]interface{}),
	}
	
	a.sessions[token] = session
	
	return session, nil
}

// generateSessionID generates a unique session ID
func (a *Authenticator) generateSessionID() string {
	bytes := make([]byte, 12)
	rand.Read(bytes)
	return base64.URLEncoding.EncodeToString(bytes)
}

// hashPassword creates a secure password hash
func (a *Authenticator) hashPassword(password string) (string, error) {
	// Generate salt
	salt := make([]byte, 32)
	if _, err := rand.Read(salt); err != nil {
		return "", err
	}
	
	// Hash password with PBKDF2 (simplified for demo)
	// In production, use bcrypt or Argon2
	hash := a.pbkdf2(password, salt, 10000)
	
	// Combine salt and hash
	combined := append(salt, hash...)
	return base64.StdEncoding.EncodeToString(combined), nil
}

// verifyPassword verifies a password against hash
func (a *Authenticator) verifyPassword(password, hash string) bool {
	// Decode combined hash
	combined, err := base64.StdEncoding.DecodeString(hash)
	if err != nil {
		return false
	}
	
	if len(combined) < 32 {
		return false
	}
	
	// Extract salt and hash
	salt := combined[:32]
	expectedHash := combined[32:]
	
	// Compute hash with same salt
	actualHash := a.pbkdf2(password, salt, 10000)
	
	// Compare using constant-time comparison
	return subtle.ConstantTimeCompare(actualHash, expectedHash) == 1
}

// pbkdf2 implements PBKDF2 key derivation (simplified)
func (a *Authenticator) pbkdf2(password string, salt []byte, iterations int) []byte {
	// Simplified PBKDF2 implementation
	// In production, use crypto/pbkdf2 package
	hash := make([]byte, 32)
	
	data := []byte(password)
	for i := 0; i < iterations; i++ {
		for _, b := range salt {
			data = append(data, b)
		}
		// Simple hash iteration (use proper implementation in production)
		for k := 0; k < len(data); k++ {
			hash[k%32] ^= data[k]
		}
	}
	
	return hash
}

// ValidatePassword checks password strength
func ValidatePassword(password string) error {
	if len(password) < 8 {
		return fmt.Errorf("password must be at least 8 characters")
	}
	
	var (
		hasUpper   bool
		hasLower   bool
		hasNumber  bool
		hasSpecial bool
	)
	
	for _, char := range password {
		switch {
		case char >= 'A' && char <= 'Z':
			hasUpper = true
		case char >= 'a' && char <= 'z':
			hasLower = true
		case char >= '0' && char <= '9':
			hasNumber = true
		case char >= 33 && char <= 126:
			hasSpecial = true
		}
	}
	
	if !hasUpper {
		return fmt.Errorf("password must contain at least one uppercase letter")
	}
	if !hasLower {
		return fmt.Errorf("password must contain at least one lowercase letter")
	}
	if !hasNumber {
		return fmt.Errorf("password must contain at least one number")
	}
	if !hasSpecial {
		return fmt.Errorf("password must contain at least one special character")
	}
	
	return nil
}