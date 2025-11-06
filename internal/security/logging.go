package security

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// SecurityEventType represents types of security events
type SecurityEventType string

const (
	// Authentication events
	EventAuthSuccess      SecurityEventType = "auth_success"
	EventAuthFailure      SecurityEventType = "auth_failure"
	EventAuthBlocked      SecurityEventType = "auth_blocked"
	
	// Permission events
	EventPermissionDenied SecurityEventType = "permission_denied"
	EventPermissionGranted SecurityEventType = "permission_granted"
	EventPermissionEscalation SecurityEventType = "permission_escalation"
	
	// Tool execution events
	EventToolExecuted    SecurityEventType = "tool_executed"
	EventToolBlocked     SecurityEventType = "tool_blocked"
	EventToolError       SecurityEventType = "tool_error"
	
	// Key management events
	EventKeyStored       SecurityEventType = "key_stored"
	EventKeyRetrieved    SecurityEventType = "key_retrieved"
	EventKeyDeleted      SecurityEventType = "key_deleted"
	EventKeyRotated      SecurityEventType = "key_rotated"
	EventKeyExposure     SecurityEventType = "key_exposure"
	
	// Network events
	EventNetworkRequest  SecurityEventType = "network_request"
	EventNetworkBlocked  SecurityEventType = "network_blocked"
	
	// File system events
	EventFileAccess     SecurityEventType = "file_access"
	EventFileBlocked    SecurityEventType = "file_blocked"
	
	// Security system events
	EventSecurityViolation SecurityEventType = "security_violation"
	EventSecurityAlert  SecurityEventType = "security_alert"
	EventSystemStart    SecurityEventType = "system_start"
	EventSystemShutdown SecurityEventType = "system_shutdown"
)

// SecuritySeverity represents event severity levels
type SecuritySeverity string

const (
	SeverityCritical SecuritySeverity = "critical"
	SeverityHigh     SecuritySeverity = "high"
	SeverityMedium   SecuritySeverity = "medium"
	SeverityLow      SecuritySeverity = "low"
	SeverityInfo     SecuritySeverity = "info"
)

// SecurityEvent represents a security event
type SecurityEvent struct {
	ID          string            `json:"id"`
	Timestamp   time.Time         `json:"timestamp"`
	Type        SecurityEventType `json:"type"`
	Severity    SecuritySeverity  `json:"severity"`
	UserID      string            `json:"user_id,omitempty"`
	SessionID   string            `json:"session_id,omitempty"`
	ToolName    string            `json:"tool_name,omitempty"`
	Description string            `json:"description"`
	Details     map[string]interface{} `json:"details,omitempty"`
	SourceIP    string            `json:"source_ip,omitempty"`
	UserAgent   string            `json:"user_agent,omitempty"`
	ProcessID   int               `json:"process_id"`
	ThreadID    int               `json:"thread_id"`
	StackHash   string            `json:"stack_hash,omitempty"`
}

// SecurityLogger provides secure audit logging
type SecurityLogger struct {
	logFile     string
	hmacKey     []byte
	mutex       sync.RWMutex
	eventBuffer []SecurityEvent
	maxBuffer   int
}

// NewSecurityLogger creates a new security logger
func NewSecurityLogger(logFile string) (*SecurityLogger, error) {
	// Ensure directory exists
	dir := filepath.Dir(logFile)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return nil, fmt.Errorf("failed to create log directory: %w", err)
	}
	
	// Generate HMAC key for log integrity
	hmacKey := make([]byte, 32)
	if _, err := rand.Read(hmacKey); err != nil {
		return nil, fmt.Errorf("failed to generate HMAC key: %w", err)
	}
	
	return &SecurityLogger{
		logFile:     logFile,
		hmacKey:     hmacKey,
		eventBuffer: make([]SecurityEvent, 0),
		maxBuffer:   1000,
	}, nil
}

// LogEvent logs a security event
func (sl *SecurityLogger) LogEvent(event SecurityEvent) error {
	sl.mutex.Lock()
	defer sl.mutex.Unlock()
	
	// Generate event ID
	event.ID = sl.generateEventID()
	event.Timestamp = time.Now()
	event.UserAgent = os.Getenv("USER_AGENT")
	event.ProcessID = os.Getpid()
	event.ThreadID = 0 // Simplified for Go
	
	// Add to buffer
	sl.eventBuffer = append(sl.eventBuffer, event)
	
	// Flush if buffer is full
	if len(sl.eventBuffer) >= sl.maxBuffer {
		return sl.flushEvents()
	}
	
	return nil
}

// LogAuthEvent logs authentication events
func (sl *SecurityLogger) LogAuthEvent(userID, sessionID string, success bool, details map[string]interface{}) {
	eventType := EventAuthFailure
	severity := SeverityHigh
	description := "Authentication failed"
	
	if success {
		eventType = EventAuthSuccess
		severity = SeverityInfo
		description = "Authentication successful"
	}
	
	event := SecurityEvent{
		Type:        eventType,
		Severity:    severity,
		UserID:      userID,
		SessionID:   sessionID,
		Description: description,
		Details:     details,
	}
	
	sl.LogEvent(event)
}

// LogPermissionEvent logs permission events
func (sl *SecurityLogger) LogPermissionEvent(userID, sessionID, toolName string, granted bool, details map[string]interface{}) {
	eventType := EventPermissionDenied
	severity := SeverityMedium
	description := fmt.Sprintf("Permission denied for tool %s", toolName)
	
	if granted {
		eventType = EventPermissionGranted
		severity = SeverityInfo
		description = fmt.Sprintf("Permission granted for tool %s", toolName)
	}
	
	event := SecurityEvent{
		Type:        eventType,
		Severity:    severity,
		UserID:      userID,
		SessionID:   sessionID,
		ToolName:    toolName,
		Description: description,
		Details:     details,
	}
	
	sl.LogEvent(event)
}

// LogToolExecution logs tool execution events
func (sl *SecurityLogger) LogToolExecution(userID, sessionID, toolName string, success bool, details map[string]interface{}) {
	eventType := EventToolExecuted
	severity := SeverityInfo
	description := fmt.Sprintf("Tool %s executed successfully", toolName)
	
	if !success {
		eventType = EventToolError
		severity = SeverityMedium
		description = fmt.Sprintf("Tool %s execution failed", toolName)
	}
	
	event := SecurityEvent{
		Type:        eventType,
		Severity:    severity,
		UserID:      userID,
		SessionID:   sessionID,
		ToolName:    toolName,
		Description: description,
		Details:     details,
	}
	
	sl.LogEvent(event)
}

// LogKeyEvent logs key management events
func (sl *SecurityLogger) LogKeyEvent(userID, sessionID, serviceName string, eventType SecurityEventType, details map[string]interface{}) {
	severity := SeverityMedium
	description := fmt.Sprintf("Key operation %s for service %s", eventType, serviceName)
	
	if eventType == EventKeyExposure {
		severity = SeverityCritical
		description = fmt.Sprintf("CRITICAL: Potential key exposure for service %s", serviceName)
	}
	
	event := SecurityEvent{
		Type:        eventType,
		Severity:    severity,
		UserID:      userID,
		SessionID:   sessionID,
		Description: description,
		Details:     details,
	}
	
	// Add service name to details
	if event.Details == nil {
		event.Details = make(map[string]interface{})
	}
	event.Details["service"] = serviceName
	
	sl.LogEvent(event)
}

// LogSecurityViolation logs security violations
func (sl *SecurityLogger) LogSecurityViolation(userID, sessionID string, violation string, details map[string]interface{}) {
	event := SecurityEvent{
		Type:        EventSecurityViolation,
		Severity:    SeverityCritical,
		UserID:      userID,
		SessionID:   sessionID,
		Description: fmt.Sprintf("Security violation: %s", violation),
		Details:     details,
	}
	
	sl.LogEvent(event)
}

// Flush writes buffered events to disk
func (sl *SecurityLogger) Flush() error {
	sl.mutex.Lock()
	defer sl.mutex.Unlock()
	
	return sl.flushEvents()
}

// flushEvents writes events to disk (internal method)
func (sl *SecurityLogger) flushEvents() error {
	if len(sl.eventBuffer) == 0 {
		return nil
	}
	
	// Create log entry with integrity check
	logEntry := map[string]interface{}{
		"timestamp": time.Now(),
		"events":    sl.eventBuffer,
		"version":   "1.0",
	}
	
	// Serialize to JSON
	jsonData, err := json.MarshalIndent(logEntry, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal events: %w", err)
	}
	
	// Generate HMAC for integrity
	hmac := hmac.New(sha256.New, sl.hmacKey)
	hmac.Write(jsonData)
	signature := base64.StdEncoding.EncodeToString(hmac.Sum(nil))
	
	// Write events with signature
	logLine := string(jsonData) + "\nHMAC:" + signature + "\n"
	
	// Append to log file with secure permissions
	file, err := os.OpenFile(sl.logFile, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
	if err != nil {
		return fmt.Errorf("failed to open log file: %w", err)
	}
	defer file.Close()
	
	if _, err := file.WriteString(logLine); err != nil {
		return fmt.Errorf("failed to write log entry: %w", err)
	}
	
	// Clear buffer
	sl.eventBuffer = sl.eventBuffer[:0]
	
	return nil
}

// generateEventID generates a unique event ID
func (sl *SecurityLogger) generateEventID() string {
	timestamp := time.Now().UnixNano()
	pid := os.Getpid()
	
	// Generate a simple unique ID
	return fmt.Sprintf("evt_%d_%d", timestamp, pid)
}

// GetRecentEvents returns recent events from buffer
func (sl *SecurityLogger) GetRecentEvents(limit int) []SecurityEvent {
	sl.mutex.RLock()
	defer sl.mutex.RUnlock()
	
	if limit <= 0 || limit > len(sl.eventBuffer) {
		limit = len(sl.eventBuffer)
	}
	
	start := len(sl.eventBuffer) - limit
	events := make([]SecurityEvent, limit)
	copy(events, sl.eventBuffer[start:])
	
	return events
}

// VerifyLogIntegrity verifies the integrity of the log file
func (sl *SecurityLogger) VerifyLogIntegrity() (bool, error) {
	// Read log file
	data, err := os.ReadFile(sl.logFile)
	if err != nil {
		if os.IsNotExist(err) {
			return true, nil // New file is valid
		}
		return false, fmt.Errorf("failed to read log file: %w", err)
	}
	
	// Split into entries
	entries := strings.Split(string(data), "\nHMAC:")
	
	for i, entry := range entries {
		if i == 0 && strings.TrimSpace(entry) == "" {
			continue
		}
		
		// Split into content and signature
		parts := strings.SplitN(entry, "\n", 2)
		if len(parts) != 2 {
			continue
		}
		
		content := parts[0]
		storedSignature := strings.TrimSpace(parts[1])
		
		// Generate HMAC for content
		hmac := hmac.New(sha256.New, sl.hmacKey)
		hmac.Write([]byte(content))
		expectedSignature := base64.StdEncoding.EncodeToString(hmac.Sum(nil))
		
		// Compare signatures
		if storedSignature != expectedSignature {
			return false, fmt.Errorf("integrity check failed for entry %d", i)
		}
	}
	
	return true, nil
}

// GetSecurityStats returns security statistics
func (sl *SecurityLogger) GetSecurityStats() map[string]int {
	sl.mutex.RLock()
	defer sl.mutex.RUnlock()
	
	stats := map[string]int{
		"total_events":    len(sl.eventBuffer),
		"auth_failures":   0,
		"permission_denied": 0,
		"tool_errors":     0,
		"security_violations": 0,
	}
	
	for _, event := range sl.eventBuffer {
		switch event.Type {
		case EventAuthFailure:
			stats["auth_failures"]++
		case EventPermissionDenied:
			stats["permission_denied"]++
		case EventToolError:
			stats["tool_errors"]++
		case EventSecurityViolation:
			stats["security_violations"]++
		}
	}
	
	return stats
}

// Global security logger instance
var globalSecurityLogger *SecurityLogger

// InitializeSecurityLogging initializes the global security logger
func InitializeSecurityLogging(logFile string) error {
	var err error
	globalSecurityLogger, err = NewSecurityLogger(logFile)
	if err != nil {
		return fmt.Errorf("failed to initialize security logging: %w", err)
	}
	
	// Log system start
	event := SecurityEvent{
		Type:        EventSystemStart,
		Severity:    SeverityInfo,
		Description: "Vaughan Crush security system started",
		Details: map[string]interface{}{
			"version": "1.0",
			"pid":     os.Getpid(),
		},
	}
	
	return globalSecurityLogger.LogEvent(event)
}

// GetSecurityLogger returns the global security logger
func GetSecurityLogger() *SecurityLogger {
	return globalSecurityLogger
}

// LogSecurityEvent is a convenience function for logging security events
func LogSecurityEvent(event SecurityEvent) error {
	if globalSecurityLogger == nil {
		return fmt.Errorf("security logger not initialized")
	}
	return globalSecurityLogger.LogEvent(event)
}