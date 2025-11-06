package security

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"regexp"
	"strings"
	"time"
)

// AIModelSecurity manages AI model security controls
type AIModelSecurity struct {
	policy          *AISecurityPolicy
	logger          *SecurityLogger
	promptHistory   map[string]*PromptInfo
	modelAccessLog  map[string]*ModelAccess
}

// AISecurityPolicy defines AI model security rules
type AISecurityPolicy struct {
	MaxPromptLength      int                    `json:"max_prompt_length"`
	MaxResponseLength   int                    `json:"max_response_length"`
	BlockedPatterns     []string               `json:"blocked_patterns"`
	AllowedModels       []string               `json:"allowed_models"`
	RateLimitPerUser    int                    `json:"rate_limit_per_user"`
	RateLimitWindow     time.Duration          `json:"rate_limit_window"`
	RequireContentFilter bool                   `json:"require_content_filter"`
	LogAllInteractions  bool                   `json:"log_all_interactions"`
	ValidateInputs      bool                   `json:"validate_inputs"`
	ValidateOutputs     bool                   `json:"validate_outputs"`
	ScanForPromptInjection bool               `json:"scan_for_prompt_injection"`
	ScanForDataLeakage bool                   `json:"scan_for_data_leakage"`
	AllowedDomains     []string               `json:"allowed_domains"`
	BlockedDomains     []string               `json:"blocked_domains"`
}

// PromptInfo represents AI prompt information
type PromptInfo struct {
	ID           string                 `json:"id"`
	UserID       string                 `json:"user_id"`
	SessionID    string                 `json:"session_id"`
	Prompt       string                 `json:"prompt"`
	Model        string                 `json:"model"`
	Timestamp    time.Time              `json:"timestamp"`
	Length       int                   `json:"length"`
	Hash         string                 `json:"hash"`
	Validated    bool                   `json:"validated"`
	Issues       []string               `json:"issues"`
	Metadata     map[string]interface{} `json:"metadata"`
}

// ModelAccess represents AI model access
type ModelAccess struct {
	ID           string                 `json:"id"`
	UserID       string                 `json:"user_id"`
	SessionID    string                 `json:"session_id"`
	Model        string                 `json:"model"`
	Operation    string                 `json:"operation"`
	Timestamp    time.Time              `json:"timestamp"`
	Duration     time.Duration          `json:"duration"`
	Success      bool                   `json:"success"`
	TokensUsed   int                    `json:"tokens_used"`
	Error        string                 `json:"error,omitempty"`
	Metadata     map[string]interface{} `json:"metadata"`
}

// PromptInjectionPattern represents prompt injection detection patterns
type PromptInjectionPattern struct {
	Name        string   `json:"name"`
	Pattern     string   `json:"pattern"`
	Severity    string   `json:"severity"`
	Description string   `json:"description"`
}

// ContentFilterResult represents content filtering results
type ContentFilterResult struct {
	Passed        bool     `json:"passed"`
	Flagged       bool     `json:"flagged"`
	Categories    []string `json:"categories"`
	Confidence    float64  `json:"confidence"`
	Details       string   `json:"details"`
	Filtered      string   `json:"filtered"`
}

// NewAIModelSecurity creates AI model security manager
func NewAIModelSecurity(policy *AISecurityPolicy, logger *SecurityLogger) *AIModelSecurity {
	return &AIModelSecurity{
		policy:         policy,
		logger:         logger,
		promptHistory:  make(map[string]*PromptInfo),
		modelAccessLog: make(map[string]*ModelAccess),
	}
}

// ValidatePrompt validates AI prompt against security policy
func (ais *AIModelSecurity) ValidatePrompt(prompt string, model string, ctx *Context) (*PromptInfo, error) {
	promptInfo := &PromptInfo{
		ID:        ais.generatePromptID(),
		UserID:    ctx.UserID,
		SessionID: ctx.SessionID,
		Prompt:    prompt,
		Model:     model,
		Timestamp: time.Now(),
		Length:    len(prompt),
		Hash:      ais.calculatePromptHash(prompt),
		Validated: false,
		Issues:    make([]string, 0),
		Metadata:  make(map[string]interface{}),
	}
	
	// Check prompt length
	if ais.policy.MaxPromptLength > 0 && len(prompt) > ais.policy.MaxPromptLength {
		promptInfo.Issues = append(promptInfo.Issues, fmt.Sprintf("prompt too long: %d > %d", len(prompt), ais.policy.MaxPromptLength))
	}
	
	// Check for blocked patterns
	if ais.policy.ScanForPromptInjection {
		if err := ais.scanPromptInjection(prompt, promptInfo); err != nil {
			promptInfo.Issues = append(promptInfo.Issues, err.Error())
		}
	}
	
	// Check for data leakage
	if ais.policy.ScanForDataLeakage {
		if err := ais.scanDataLeakage(prompt, promptInfo); err != nil {
			promptInfo.Issues = append(promptInfo.Issues, err.Error())
		}
	}
	
	// Check allowed models
	if len(ais.policy.AllowedModels) > 0 {
		allowed := false
		for _, allowedModel := range ais.policy.AllowedModels {
			if model == allowedModel {
				allowed = true
				break
			}
		}
		if !allowed {
			promptInfo.Issues = append(promptInfo.Issues, fmt.Sprintf("model '%s' not allowed", model))
		}
	}
	
	// Check rate limiting
	if err := ais.checkRateLimit(ctx.UserID); err != nil {
		promptInfo.Issues = append(promptInfo.Issues, err.Error())
	}
	
	// Validate prompt content
	if ais.policy.ValidateInputs {
		if err := ais.validatePromptContent(prompt, promptInfo); err != nil {
			promptInfo.Issues = append(promptInfo.Issues, err.Error())
		}
	}
	
	// Mark as validated if no issues
	if len(promptInfo.Issues) == 0 {
		promptInfo.Validated = true
	}
	
	// Log prompt validation
	if ais.logger != nil {
		eventType := SecurityEventType("prompt_validated")
		severity := SeverityInfo
		
		if promptInfo.Validated {
			eventType = SecurityEventType("prompt_validated")
			severity = SeverityInfo
		}
		
		event := SecurityEvent{
			Type:        eventType,
			Severity:    severity,
			UserID:      ctx.UserID,
			SessionID:   ctx.SessionID,
			Description: fmt.Sprintf("AI prompt validation: %s", model),
			Details: map[string]interface{}{
				"prompt_id":   promptInfo.ID,
				"model":       model,
				"length":      promptInfo.Length,
				"validated":   promptInfo.Validated,
				"issues":      promptInfo.Issues,
			},
		}
		
		ais.logger.LogEvent(event)
	}
	
	// Store prompt history
	ais.promptHistory[promptInfo.ID] = promptInfo
	
	if !promptInfo.Validated {
		return promptInfo, fmt.Errorf("prompt validation failed: %s", strings.Join(promptInfo.Issues, ", "))
	}
	
	return promptInfo, nil
}

// SanitizeResponse sanitizes AI model response
func (ais *AIModelSecurity) SanitizeResponse(response string, promptID string, ctx *Context) (*ContentFilterResult, error) {
	result := &ContentFilterResult{
		Passed:     true,
		Flagged:    false,
		Categories: make([]string, 0),
		Confidence: 1.0,
		Details:    "",
		Filtered:   response,
	}
	
	// Check response length
	if ais.policy.MaxResponseLength > 0 && len(response) > ais.policy.MaxResponseLength {
		result.Passed = false
		result.Flagged = true
		result.Categories = append(result.Categories, "length_exceeded")
		result.Details = fmt.Sprintf("response too long: %d > %d", len(response), ais.policy.MaxResponseLength)
	}
	
	// Check for blocked content patterns
	for _, pattern := range ais.policy.BlockedPatterns {
		if matched, _ := regexp.MatchString(pattern, response); matched {
			result.Passed = false
			result.Flagged = true
			result.Categories = append(result.Categories, "blocked_pattern")
			result.Details = fmt.Sprintf("blocked pattern detected: %s", pattern)
			break
		}
	}
	
	// Check for data leakage
	if ais.policy.ScanForDataLeakage {
		if err := ais.scanDataLeakage(response, nil); err != nil {
			result.Passed = false
			result.Flagged = true
			result.Categories = append(result.Categories, "data_leakage")
			result.Details = err.Error()
		}
	}
	
	// Apply content filtering if required
	if ais.policy.RequireContentFilter && result.Flagged {
		result.Filtered = ais.filterContent(response)
	}
	
	// Log response sanitization
	if ais.logger != nil {
		eventType := SecurityEventType("response_sanitized")
		severity := SeverityInfo
		
		if result.Flagged {
			eventType = SecurityEventType(EventSecurityViolation)
			severity = SeverityHigh
		}
		
		event := SecurityEvent{
			Type:        eventType,
			Severity:    severity,
			UserID:      ctx.UserID,
			SessionID:   ctx.SessionID,
			Description: "AI response sanitization",
			Details: map[string]interface{}{
				"prompt_id":  promptID,
				"passed":     result.Passed,
				"flagged":    result.Flagged,
				"categories": result.Categories,
				"details":    result.Details,
			},
		}
		
		ais.logger.LogEvent(event)
	}
	
	return result, nil
}

// LogModelAccess logs AI model access
func (ais *AIModelSecurity) LogModelAccess(model, operation string, duration time.Duration, tokensUsed int, success bool, errMsg string, ctx *Context) {
	access := &ModelAccess{
		ID:         ais.generateAccessID(),
		UserID:     ctx.UserID,
		SessionID:  ctx.SessionID,
		Model:      model,
		Operation:  operation,
		Timestamp:  time.Now(),
		Duration:   duration,
		Success:    success,
		TokensUsed: tokensUsed,
		Error:      errMsg,
		Metadata:   make(map[string]interface{}),
	}
	
	ais.modelAccessLog[access.ID] = access
	
	// Log model access
	if ais.logger != nil {
		eventType := SecurityEventType("model_access")
		severity := SeverityInfo
		
		if success {
			eventType = SecurityEventType("model_access")
			severity = SeverityInfo
		} else {
			eventType = SecurityEventType(EventSecurityViolation)
			severity = SeverityMedium
		}
		
		event := SecurityEvent{
			Type:        eventType,
			Severity:    severity,
			UserID:      ctx.UserID,
			SessionID:   ctx.SessionID,
			Description: fmt.Sprintf("AI model access: %s", model),
			Details: map[string]interface{}{
				"access_id":    access.ID,
				"model":        model,
				"operation":    operation,
				"duration":     duration.String(),
				"tokens_used":  tokensUsed,
				"success":      success,
				"error":        errMsg,
			},
		}
		
		ais.logger.LogEvent(event)
	}
}

// scanPromptInjection scans for prompt injection attacks
func (ais *AIModelSecurity) scanPromptInjection(prompt string, promptInfo *PromptInfo) error {
	patterns := []PromptInjectionPattern{
		{
			Name:        "Ignore Previous Instructions",
			Pattern:     `(?i)(ignore|disregard|forget).*(previous|earlier|earlier).*(instruction|instructions|prompt|prompts)`,
			Severity:    "high",
			Description: "Attempt to ignore previous instructions",
		},
		{
			Name:        "System Prompt Override",
			Pattern:     `(?i)(you are|act as|pretend to be).*(ai|assistant|model|system)`,
			Severity:    "high",
			Description: "Attempt to override system prompts",
		},
		{
			Name:        "Role Playing",
			Pattern:     `(?i)(role play|roleplay|role-play).*(as|being|a).*(jailbreak|uncensored|unfiltered)`,
			Severity:    "medium",
			Description: "Attempt to engage in unauthorized role playing",
		},
		{
			Name:        "Code Execution",
			Pattern:     `(?i)(execute|run|exec).*(code|command|script|program)`,
			Severity:    "high",
			Description: "Attempt to execute code or commands",
		},
		{
			Name:        "Information Disclosure",
			Pattern:     `(?i)(tell me|show me|give me).*(your|system|model).*(instructions|prompts|configuration|secrets)`,
			Severity:    "medium",
			Description: "Attempt to extract system information",
		},
	}
	
	for _, pattern := range patterns {
		if matched, _ := regexp.MatchString(pattern.Pattern, prompt); matched {
			promptInfo.Issues = append(promptInfo.Issues, fmt.Sprintf("prompt injection detected: %s (%s)", pattern.Name, pattern.Severity))
			return fmt.Errorf("prompt injection pattern detected: %s", pattern.Name)
		}
	}
	
	return nil
}

// scanDataLeakage scans for potential data leakage
func (ais *AIModelSecurity) scanDataLeakage(text string, promptInfo *PromptInfo) error {
	// Pattern for sensitive data
	sensitivePatterns := []struct {
		Name    string
		Pattern string
	}{
		{"API Key", `(?i)(api[_-]?key|apikey)[\s:=]+['\"]?[a-zA-Z0-9\-_]{20,}['\"]?`},
		{"Private Key", `(?i)(private[_-]?key|privatekey)[\s:=]+['\"]?[a-fA-F0-9]{64}['\"]?`},
		{"Password", `(?i)(password|pwd|pass)[\s:=]+['\"]?[^\s'\"]{6,}['\"]?`},
		{"Email Address", `[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}`},
		{"Phone Number", `\+?1?-?\.?\s?\(?([0-9]{3})\)?[-.\s]?([0-9]{3})[-.\s]?([0-9]{4})`},
		{"Credit Card", `\b(?:4[0-9]{12}(?:[0-9]{3})?|5[1-5][0-9]{14}|3[47][0-9]{13}|3[0-9]{13}|6(?:011|5[0-9]{2})[0-9]{12})\b`},
		{"SSN", `\b\d{3}-\d{2}-\d{4}\b`},
	}
	
	for _, pattern := range sensitivePatterns {
		if matched, _ := regexp.MatchString(pattern.Pattern, text); matched {
			if promptInfo != nil {
				promptInfo.Issues = append(promptInfo.Issues, fmt.Sprintf("potential data leakage: %s detected", pattern.Name))
			}
			return fmt.Errorf("potential data leakage detected: %s", pattern.Name)
		}
	}
	
	return nil
}

// validatePromptContent validates prompt content
func (ais *AIModelSecurity) validatePromptContent(prompt string, promptInfo *PromptInfo) error {
	// Check for empty prompt
	if strings.TrimSpace(prompt) == "" {
		promptInfo.Issues = append(promptInfo.Issues, "prompt cannot be empty")
		return fmt.Errorf("prompt cannot be empty")
	}
	
	// Check for excessive whitespace
	if len(strings.TrimSpace(prompt)) < len(prompt)/2 {
		promptInfo.Issues = append(promptInfo.Issues, "prompt contains excessive whitespace")
	}
	
	// Check for repeated characters
	if ais.hasExcessiveRepetition(prompt) {
		promptInfo.Issues = append(promptInfo.Issues, "prompt contains excessive repetition")
	}
	
	return nil
}

// hasExcessiveRepetition checks for excessive character repetition
func (ais *AIModelSecurity) hasExcessiveRepetition(text string) bool {
	// Simple repetition detection
	threshold := 5
	charCount := make(map[rune]int)
	
	for _, char := range text {
		charCount[char]++
		if charCount[char] > threshold {
			return true
		}
	}
	
	return false
}

// filterContent filters sensitive content from response
func (ais *AIModelSecurity) filterContent(response string) string {
	// Simple content filtering
	// In production, use advanced content filtering
	filtered := response
	
	// Replace potential sensitive patterns
	sensitivePatterns := []string{
		`(?i)(api[_-]?key|apikey)[\s:=]+['\"]?[a-zA-Z0-9\-_]{20,}['\"]?`,
		`(?i)(private[_-]?key|privatekey)[\s:=]+['\"]?[a-fA-F0-9]{64}['\"]?`,
		`(?i)(password|pwd|pass)[\s:=]+['\"]?[^\s'\"]{6,}['\"]?`,
		`[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}`,
		`\b\d{3}-\d{2}-\d{4}\b`,
	}
	
	for _, pattern := range sensitivePatterns {
		re := regexp.MustCompile(pattern)
		filtered = re.ReplaceAllString(filtered, "[FILTERED]")
	}
	
	return filtered
}

// checkRateLimit implements user rate limiting
func (ais *AIModelSecurity) checkRateLimit(userID string) error {
	if ais.policy.RateLimitPerUser <= 0 {
		return nil
	}
	
	// Simple rate limiting check
	// In production, use proper rate limiting with Redis/DB
	now := time.Now()
	windowStart := now.Add(-ais.policy.RateLimitWindow)
	
	count := 0
	for _, prompt := range ais.promptHistory {
		if prompt.UserID == userID && prompt.Timestamp.After(windowStart) {
			count++
		}
	}
	
	if count >= ais.policy.RateLimitPerUser {
		return fmt.Errorf("rate limit exceeded: %d requests per %v", ais.policy.RateLimitPerUser, ais.policy.RateLimitWindow)
	}
	
	return nil
}

// calculatePromptHash calculates prompt hash for deduplication
func (ais *AIModelSecurity) calculatePromptHash(prompt string) string {
	hash := sha256.Sum256([]byte(prompt))
	return hex.EncodeToString(hash[:])
}

// generatePromptID generates unique prompt ID
func (ais *AIModelSecurity) generatePromptID() string {
	return fmt.Sprintf("prompt_%d", time.Now().UnixNano())
}

// generateAccessID generates unique access ID
func (ais *AIModelSecurity) generateAccessID() string {
	return fmt.Sprintf("access_%d", time.Now().UnixNano())
}

// GetPromptHistory returns prompt history for user
func (ais *AIModelSecurity) GetPromptHistory(userID string, limit int) []*PromptInfo {
	var history []*PromptInfo
	
	for _, prompt := range ais.promptHistory {
		if prompt.UserID == userID {
			history = append(history, prompt)
		}
	}
	
	// Sort by timestamp (most recent first)
	// In production, implement proper sorting
	
	if limit > 0 && len(history) > limit {
		history = history[:limit]
	}
	
	return history
}

// GetModelAccessLog returns model access log for user
func (ais *AIModelSecurity) GetModelAccessLog(userID string, limit int) []*ModelAccess {
	var log []*ModelAccess
	
	for _, access := range ais.modelAccessLog {
		if access.UserID == userID {
			log = append(log, access)
		}
	}
	
	// Sort by timestamp (most recent first)
	// In production, implement proper sorting
	
	if limit > 0 && len(log) > limit {
		log = log[:limit]
	}
	
	return log
}

// DefaultAISecurityPolicy returns secure default AI security policy
func DefaultAISecurityPolicy() *AISecurityPolicy {
	return &AISecurityPolicy{
		MaxPromptLength:           4000,
		MaxResponseLength:          8000,
		BlockedPatterns:           []string{},
		AllowedModels:             []string{"gpt-3.5-turbo", "gpt-4", "claude-2", "claude-instant"},
		RateLimitPerUser:          60,
		RateLimitWindow:           time.Hour,
		RequireContentFilter:       true,
		LogAllInteractions:        true,
		ValidateInputs:            true,
		ValidateOutputs:           true,
		ScanForPromptInjection:    true,
		ScanForDataLeakage:       true,
		AllowedDomains:            []string{"api.openai.com", "api.anthropic.com"},
		BlockedDomains:            []string{},
	}
}

// RestrictiveAISecurityPolicy returns highly restrictive AI security policy
func RestrictiveAISecurityPolicy() *AISecurityPolicy {
	return &AISecurityPolicy{
		MaxPromptLength:           1000,
		MaxResponseLength:          2000,
		BlockedPatterns:           []string{`(?i)(password|key|secret|token)`},
		AllowedModels:             []string{"gpt-3.5-turbo"},
		RateLimitPerUser:          20,
		RateLimitWindow:           time.Hour,
		RequireContentFilter:       true,
		LogAllInteractions:        true,
		ValidateInputs:            true,
		ValidateOutputs:           true,
		ScanForPromptInjection:    true,
		ScanForDataLeakage:       true,
		AllowedDomains:            []string{"api.openai.com"},
		BlockedDomains:            []string{"pastebin.com", "github.com"},
	}
}