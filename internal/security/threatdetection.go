package security

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"regexp"
	"strings"
	"sync"
	"time"
)

// ThreatLevel represents current threat severity
type ThreatLevel string

const (
	ThreatLevelLow      ThreatLevel = "low"
	ThreatLevelMedium   ThreatLevel = "medium"
	ThreatLevelHigh     ThreatLevel = "high"
	ThreatLevelCritical ThreatLevel = "critical"
)

// ThreatType represents different types of security threats
type ThreatType string

const (
	ThreatTypeBruteForce    ThreatType = "brute_force"
	ThreatTypeInjection     ThreatType = "injection"
	ThreatTypeXSS          ThreatType = "xss"
	ThreatTypeCSRF         ThreatType = "csrf"
	ThreatTypeSQLInjection ThreatType = "sql_injection"
	ThreatTypeDDoS         ThreatType = "ddos"
	ThreatTypeMalware      ThreatType = "malware"
	ThreatTypeDataBreach   ThreatType = "data_breach"
	ThreatTypePrivEscalation ThreatType = "privilege_escalation"
	ThreatTypeReconnaissance ThreatType = "reconnaissance"
	ThreatTypeAnomaly      ThreatType = "anomaly"
)

// Threat represents a detected security threat
type Threat struct {
	ID            string                 `json:"id"`
	Type          ThreatType             `json:"type"`
	Level         ThreatLevel            `json:"level"`
	Title         string                 `json:"title"`
	Description   string                 `json:"description"`
	Timestamp     time.Time              `json:"timestamp"`
	SourceIP      string                 `json:"source_ip"`
	UserID        string                 `json:"user_id"`
	SessionID     string                 `json:"session_id"`
	Endpoint      string                 `json:"endpoint"`
	Payload       map[string]interface{} `json:"payload"`
	Indicators    []ThreatIndicator      `json:"indicators"`
	Score         float64                `json:"score"`
	Confidence    float64                `json:"confidence"`
	Status        ThreatStatus           `json:"status"`
	Mitigated     bool                   `json:"mitigated"`
	Mitigation    string                 `json:"mitigation,omitempty"`
	Metadata      map[string]interface{} `json:"metadata"`
}

// ThreatIndicator represents evidence of a threat
type ThreatIndicator struct {
	Type        string      `json:"type"`
	Value       interface{} `json:"value"`
	Description string      `json:"description"`
	Severity    string      `json:"severity"`
	Confidence  float64     `json:"confidence"`
}

// ThreatStatus represents threat lifecycle status
type ThreatStatus string

const (
	ThreatStatusActive      ThreatStatus = "active"
	ThreatStatusInvestigating ThreatStatus = "investigating"
	ThreatStatusMitigated   ThreatStatus = "mitigated"
	ThreatStatusFalsePositive ThreatStatus = "false_positive"
	ThreatStatusResolved    ThreatStatus = "resolved"
)

// ThreatDetectionRule represents a threat detection rule
type ThreatDetectionRule struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Type        ThreatType             `json:"type"`
	Description string                 `json:"description"`
	Enabled     bool                   `json:"enabled"`
	Priority    int                    `json:"priority"`
	Conditions  []DetectionCondition    `json:"conditions"`
	Actions     []ThreatResponseAction  `json:"actions"`
	Threshold   DetectionThreshold      `json:"threshold"`
	Window      time.Duration          `json:"window"`
	CreatedAt   time.Time              `json:"created_at"`
	UpdatedAt   time.Time              `json:"updated_at"`
}

// DetectionCondition represents a threat detection condition
type DetectionCondition struct {
	Field     string      `json:"field"`
	Operator  string      `json:"operator"`
	Value     interface{} `json:"value"`
	Negate    bool        `json:"negate"`
	Weight    float64     `json:"weight"`
}

// DetectionThreshold represents detection thresholds
type DetectionThreshold struct {
	Count       int         `json:"count"`
	Score       float64     `json:"score"`
	TimeWindow  time.Duration `json:"time_window"`
	Confidence  float64     `json:"confidence"`
}

// ThreatResponseAction represents automatic threat response actions
type ThreatResponseAction struct {
	Type        string                 `json:"type"`
	Parameters  map[string]interface{} `json:"parameters"`
	Delay       time.Duration          `json:"delay"`
	Enabled     bool                   `json:"enabled"`
	Description string                 `json:"description"`
}

// ThreatEngine provides advanced threat detection capabilities
type ThreatEngine struct {
	rules         map[string]*ThreatDetectionRule
	activeThreats map[string]*Threat
	threatHistory []Threat
	indicators    map[string][]ThreatIndicator
	mutex         sync.RWMutex
	logger        *SecurityLogger
	mlModel       *MLThreatDetector
	database      *ThreatDatabase
}

// MLThreatDetector represents machine learning threat detection
type MLThreatDetector struct {
	modelPath string
	enabled  bool
	threshold float64
}

// ThreatDatabase provides threat intelligence
type ThreatDatabase struct {
	indicators map[string]ThreatIndicator
	signatures map[string]string
	iocs       map[string]interface{}
	lastUpdate time.Time
}

// NewThreatEngine creates a new threat detection engine
func NewThreatEngine(logger *SecurityLogger) *ThreatEngine {
	te := &ThreatEngine{
		rules:         make(map[string]*ThreatDetectionRule),
		activeThreats: make(map[string]*Threat),
		threatHistory: make([]Threat, 0),
		indicators:    make(map[string][]ThreatIndicator),
		logger:        logger,
		mlModel:       NewMLThreatDetector("models/threat_detector.bin", 0.7),
		database:      NewThreatDatabase(),
	}
	
	// Initialize default threat detection rules
	te.initializeDefaultRules()
	
	return te
}

// NewMLThreatDetector creates a machine learning threat detector
func NewMLThreatDetector(modelPath string, threshold float64) *MLThreatDetector {
	return &MLThreatDetector{
		modelPath: modelPath,
		enabled:   true,
		threshold: threshold,
	}
}

// NewThreatDatabase creates threat intelligence database
func NewThreatDatabase() *ThreatDatabase {
	td := &ThreatDatabase{
		indicators: make(map[string][]ThreatIndicator),
		signatures: make(map[string]string),
		iocs:       make(map[string]interface{}),
		lastUpdate: time.Now(),
	}
	
	// Initialize threat intelligence
	td.initializeThreatIntelligence()
	
	return td
}

// AnalyzeEvent analyzes security event for threats
func (te *ThreatEngine) AnalyzeEvent(event SecurityEvent) []Threat {
	var threats []Threat
	
	te.mutex.Lock()
	defer te.mutex.Unlock()
	
	// Check each enabled rule
	for _, rule := range te.rules {
		if !rule.Enabled {
			continue
		}
		
		threat := te.evaluateRule(rule, event)
		if threat != nil {
			threats = append(threats, *threat)
		}
	}
	
	// Perform ML-based detection
	if te.mlModel.enabled {
		mlThreats := te.mlModel.AnalyzeEvent(event)
		threats = append(threats, mlThreats...)
	}
	
	// Check against threat database
	dbThreats := te.database.CheckEvent(event)
	threats = append(threats, dbThreats...)
	
	// Process and store detected threats
	for _, threat := range threats {
		te.processThreat(threat)
	}
	
	return threats
}

// AnalyzeNetworkTraffic analyzes network traffic for threats
func (te *ThreatEngine) AnalyzeNetworkTraffic(sourceIP, destIP string, port int, payload []byte, protocol string, ctx *Context) []Threat {
	var threats []Threat
	
	// Check for malicious IPs
	if te.database.IsMaliciousIP(sourceIP) {
		threat := Threat{
			ID:          te.generateThreatID(),
			Type:        ThreatTypeReconnaissance,
			Level:       ThreatLevelHigh,
			Title:       "Malicious IP detected",
			Description: fmt.Sprintf("Connection from known malicious IP: %s", sourceIP),
			Timestamp:   time.Now(),
			SourceIP:    sourceIP,
			UserID:      ctx.UserID,
			SessionID:   ctx.SessionID,
			Endpoint:    fmt.Sprintf("%s:%d", destIP, port),
			Score:       8.0,
			Confidence:  0.9,
			Status:      ThreatStatusActive,
			Indicators: []ThreatIndicator{
				{
					Type:        "malicious_ip",
					Value:       sourceIP,
					Description: "Source IP found in threat intelligence database",
					Severity:    "high",
					Confidence:  0.9,
				},
			},
			Metadata: map[string]interface{}{
				"protocol": protocol,
				"payload_size": len(payload),
			},
		}
		threats = append(threats, threat)
	}
	
	// Check for suspicious patterns in payload
	if payload != nil {
		payloadStr := string(payload)
		
		// SQL injection patterns
		sqlPatterns := []string{
			"(?i)(union|select|insert|update|delete|drop|create|alter)\\s+",
			"(?i)(or|and)\\s+['\"][^'\"]*['\"]\\s*=\\s*['\"][^'\"]*['\"]",
			"(?i)(exec|execute)\\s*\\(",
			"(?i)(xp_|sp_)",
		}
		
		for _, pattern := range sqlPatterns {
			if matched, _ := regexp.MatchString(pattern, payloadStr); matched {
				threat := Threat{
					ID:          te.generateThreatID(),
					Type:        ThreatTypeSQLInjection,
					Level:       ThreatLevelHigh,
					Title:       "SQL Injection detected",
					Description: fmt.Sprintf("SQL injection pattern detected in payload: %s", pattern),
					Timestamp:   time.Now(),
					SourceIP:    sourceIP,
					UserID:      ctx.UserID,
					SessionID:   ctx.SessionID,
					Endpoint:    fmt.Sprintf("%s:%d", destIP, port),
					Score:       7.5,
					Confidence:  0.8,
					Status:      ThreatStatusActive,
					Indicators: []ThreatIndicator{
						{
							Type:        "sql_injection",
							Value:       pattern,
							Description: "SQL injection pattern detected",
							Severity:    "high",
							Confidence:  0.8,
						},
					},
					Metadata: map[string]interface{}{
						"protocol": protocol,
						"payload": strings.TrimSpace(payloadStr),
					},
				}
				threats = append(threats, threat)
				break
			}
		}
		
		// XSS patterns
		xssPatterns := []string{
			"(?i)<script[^>]*>.*?</script>",
			"(?i)javascript:",
			"(?i)on\\w+\\s*=",
			"(?i)<iframe[^>]*>.*?</iframe>",
		}
		
		for _, pattern := range xssPatterns {
			if matched, _ := regexp.MatchString(pattern, payloadStr); matched {
				threat := Threat{
					ID:          te.generateThreatID(),
					Type:        ThreatTypeXSS,
					Level:       ThreatLevelMedium,
					Title:       "XSS detected",
					Description: fmt.Sprintf("Cross-site scripting pattern detected: %s", pattern),
					Timestamp:   time.Now(),
					SourceIP:    sourceIP,
					UserID:      ctx.UserID,
					SessionID:   ctx.SessionID,
					Endpoint:    fmt.Sprintf("%s:%d", destIP, port),
					Score:       6.0,
					Confidence:  0.7,
					Status:      ThreatStatusActive,
					Indicators: []ThreatIndicator{
						{
							Type:        "xss",
							Value:       pattern,
							Description: "XSS pattern detected",
							Severity:    "medium",
							Confidence:  0.7,
						},
					},
					Metadata: map[string]interface{}{
						"protocol": protocol,
						"payload": strings.TrimSpace(payloadStr),
					},
				}
				threats = append(threats, threat)
				break
			}
		}
	}
	
	// Process detected threats
	for _, threat := range threats {
		te.processThreat(threat)
	}
	
	return threats
}

// AnalyzeUserBehavior analyzes user behavior for anomalies
func (te *ThreatEngine) AnalyzeUserBehavior(userID string, actions []UserAction) []Threat {
	var threats []Threat
	
	// Analyze behavior patterns
	if te.mlModel.enabled {
		anomalies := te.mlModel.AnalyzeBehavior(userID, actions)
		for _, anomaly := range anomalies {
			threat := Threat{
				ID:          te.generateThreatID(),
				Type:        ThreatTypeAnomaly,
				Level:       ThreatLevelMedium,
				Title:       "User behavior anomaly detected",
				Description: fmt.Sprintf("Unusual behavior pattern detected: %s", anomaly.Description),
				Timestamp:   time.Now(),
				UserID:      userID,
				Score:       anomaly.Score,
				Confidence:  anomaly.Confidence,
				Status:      ThreatStatusActive,
				Indicators: []ThreatIndicator{
					{
						Type:        "behavior_anomaly",
						Value:       anomaly.Description,
						Description: "Behavior anomaly detected",
						Severity:    "medium",
						Confidence:  anomaly.Confidence,
					},
				},
				Metadata: map[string]interface{}{
					"anomaly_type": anomaly.Type,
					"pattern":      anomaly.Pattern,
				},
			}
			threats = append(threats, threat)
		}
	}
	
	// Process detected threats
	for _, threat := range threats {
		te.processThreat(threat)
	}
	
	return threats
}

// AddRule adds a threat detection rule
func (te *ThreatEngine) AddRule(rule *ThreatDetectionRule) error {
	te.mutex.Lock()
	defer te.mutex.Unlock()
	
	rule.CreatedAt = time.Now()
	rule.UpdatedAt = time.Now()
	te.rules[rule.ID] = rule
	
	// Log rule addition
	if te.logger != nil {
		te.logger.LogThreatRuleAdd(rule.ID, rule.Name, "rule_added")
	}
	
	return nil
}

// RemoveRule removes a threat detection rule
func (te *ThreatEngine) RemoveRule(ruleID string) error {
	te.mutex.Lock()
	defer te.mutex.Unlock()
	
	delete(te.rules, ruleID)
	
	// Log rule removal
	if te.logger != nil {
		te.logger.LogThreatRuleAdd(ruleID, "", "rule_removed")
	}
	
	return nil
}

// GetActiveThreats returns currently active threats
func (te *ThreatEngine) GetActiveThreats() []Threat {
	te.mutex.RLock()
	defer te.mutex.RUnlock()
	
	var threats []Threat
	for _, threat := range te.activeThreats {
		if threat.Status == ThreatStatusActive {
			threats = append(threats, *threat)
		}
	}
	
	return threats
}

// MitigateThreat mitigates a detected threat
func (te *ThreatEngine) MitigateThreat(threatID, mitigation string, ctx *Context) error {
	te.mutex.Lock()
	defer te.mutex.Unlock()
	
	threat, exists := te.activeThreats[threatID]
	if !exists {
		return fmt.Errorf("threat not found: %s", threatID)
	}
	
	// Apply mitigation
	threat.Status = ThreatStatusMitigated
	threat.Mitigated = true
	threat.Mitigation = mitigation
	
	// Log mitigation
	if te.logger != nil {
		te.logger.LogThreatMitigation(threatID, mitigation, ctx.UserID, ctx.SessionID)
	}
	
	return nil
}

// GetThreatStatistics returns threat statistics
func (te *ThreatEngine) GetThreatStatistics() ThreatStatistics {
	te.mutex.RLock()
	defer te.mutex.RUnlock()
	
	stats := ThreatStatistics{
		TotalThreats:     len(te.threatHistory) + len(te.activeThreats),
		ActiveThreats:    0,
		MitigatedThreats:  0,
		ThreatsByType:     make(map[ThreatType]int),
		ThreatsByLevel:    make(map[ThreatLevel]int),
		AverageScore:       0.0,
		AverageConfidence:  0.0,
		RecentTrends:      make(map[ThreatType]int),
	}
	
	totalScore := 0.0
	totalConfidence := 0.0
	
	// Count active threats
	for _, threat := range te.activeThreats {
		if threat.Status == ThreatStatusActive {
			stats.ActiveThreats++
			stats.ThreatsByType[threat.Type]++
			stats.ThreatsByLevel[threat.Level]++
			totalScore += threat.Score
			totalConfidence += threat.Confidence
		}
		if threat.Status == ThreatStatusMitigated {
			stats.MitigatedThreats++
		}
	}
	
	// Count historical threats
	for _, threat := range te.threatHistory {
		stats.ThreatsByType[threat.Type]++
		stats.ThreatsByLevel[threat.Level]++
		
		// Recent trends (last 24 hours)
		if time.Since(threat.Timestamp) <= 24*time.Hour {
			stats.RecentTrends[threat.Type]++
		}
	}
	
	// Calculate averages
	totalThreats := stats.ActiveThreats
	if totalThreats > 0 {
		stats.AverageScore = totalScore / float64(totalThreats)
		stats.AverageConfidence = totalConfidence / float64(totalThreats)
	}
	
	return stats
}

// UserAction represents a user action for behavior analysis
type UserAction struct {
	Timestamp   time.Time              `json:"timestamp"`
	Action      string                 `json:"action"`
	Resource    string                 `json:"resource"`
	IPAddress   string                 `json:"ip_address"`
	UserAgent   string                 `json:"user_agent"`
	Success     bool                   `json:"success"`
	Details     map[string]interface{} `json:"details"`
}

// BehaviorAnomaly represents a detected behavior anomaly
type BehaviorAnomaly struct {
	Type        string  `json:"type"`
	Description string  `json:"description"`
	Pattern     string  `json:"pattern"`
	Score       float64 `json:"score"`
	Confidence  float64 `json:"confidence"`
}

// ThreatStatistics represents threat detection statistics
type ThreatStatistics struct {
	TotalThreats     int                      `json:"total_threats"`
	ActiveThreats    int                      `json:"active_threats"`
	MitigatedThreats  int                      `json:"mitigated_threats"`
	ThreatsByType     map[ThreatType]int       `json:"threats_by_type"`
	ThreatsByLevel    map[ThreatLevel]int      `json:"threats_by_level"`
	AverageScore       float64                  `json:"average_score"`
	AverageConfidence  float64                  `json:"average_confidence"`
	RecentTrends      map[ThreatType]int       `json:"recent_trends"`
}

// evaluateRule evaluates a detection rule against an event
func (te *ThreatEngine) evaluateRule(rule *ThreatDetectionRule, event SecurityEvent) *Threat {
	score := 0.0
	confidence := 0.0
	conditionsMet := 0
	
	// Evaluate each condition
	for _, condition := range rule.Conditions {
		if te.evaluateCondition(condition, event) {
			score += condition.Weight
			conditionsMet++
		} else if condition.Negate {
			score += condition.Weight
			conditionsMet++
		}
	}
	
	// Check if threshold is met
	if conditionsMet >= rule.Threshold.Count && score >= rule.Threshold.Score {
		threat := Threat{
			ID:          te.generateThreatID(),
			Type:        rule.Type,
			Level:       te.determineThreatLevel(score),
			Title:       rule.Name,
			Description: rule.Description,
			Timestamp:   time.Now(),
			UserID:      event.UserID,
			SessionID:   event.SessionID,
			Endpoint:    event.Details["endpoint"].(string),
			Score:       score,
			Confidence:  float64(conditionsMet) / float64(len(rule.Conditions)),
			Status:      ThreatStatusActive,
			Indicators:  te.generateIndicators(rule, event),
			Metadata: map[string]interface{}{
				"rule_id":   rule.ID,
				"rule_name": rule.Name,
				"event_id":  event.ID,
			},
		}
		
		return &threat
	}
	
	return nil
}

// evaluateCondition evaluates a single detection condition
func (te *ThreatEngine) evaluateCondition(condition DetectionCondition, event SecurityEvent) bool {
	var eventValue interface{}
	
	// Extract field value from event
	switch condition.Field {
	case "type":
		eventValue = string(event.Type)
	case "severity":
		eventValue = string(event.Severity)
	case "user_id":
		eventValue = event.UserID
	case "session_id":
		eventValue = event.SessionID
	case "timestamp":
		eventValue = event.Timestamp
	default:
		if val, exists := event.Details[condition.Field]; exists {
			eventValue = val
		} else {
			return false
		}
	}
	
	// Evaluate condition based on operator
	return te.evaluateOperator(eventValue, condition.Operator, condition.Value, condition.Negate)
}

// evaluateOperator evaluates condition operator
func (te *ThreatEngine) evaluateOperator(eventValue interface{}, operator string, conditionValue interface{}, negate bool) bool {
	result := false
	
	switch operator {
	case "equals", "==":
		result = eventValue == conditionValue
	case "not_equals", "!=":
		result = eventValue != conditionValue
	case "contains":
		if eventStr, ok := eventValue.(string); ok {
			if condStr, ok := conditionValue.(string); ok {
				result = strings.Contains(eventStr, condStr)
			}
		}
	case "not_contains":
		if eventStr, ok := eventValue.(string); ok {
			if condStr, ok := conditionValue.(string); ok {
				result = !strings.Contains(eventStr, condStr)
			}
		}
	case "greater_than", ">":
		if eventNum, ok := eventValue.(float64); ok {
			if condNum, ok := conditionValue.(float64); ok {
				result = eventNum > condNum
			}
		}
	case "less_than", "<":
		if eventNum, ok := eventValue.(float64); ok {
			if condNum, ok := conditionValue.(float64); ok {
				result = eventNum < condNum
			}
		}
	case "regex":
		if eventStr, ok := eventValue.(string); ok {
			if condStr, ok := conditionValue.(string); ok {
				matched, _ := regexp.MatchString(condStr, eventStr)
				result = matched
			}
		}
	}
	
	if negate {
		result = !result
	}
	
	return result
}

// determineThreatLevel determines threat level from score
func (te *ThreatEngine) determineThreatLevel(score float64) ThreatLevel {
	if score >= 8.0 {
		return ThreatLevelCritical
	} else if score >= 6.0 {
		return ThreatLevelHigh
	} else if score >= 4.0 {
		return ThreatLevelMedium
	} else {
		return ThreatLevelLow
	}
}

// generateIndicators generates threat indicators from rule evaluation
func (te *ThreatEngine) generateIndicators(rule *ThreatDetectionRule, event SecurityEvent) []ThreatIndicator {
	var indicators []ThreatIndicator
	
	for _, condition := range rule.Conditions {
		if te.evaluateCondition(condition, event) {
			indicator := ThreatIndicator{
				Type:        condition.Field,
				Value:       event.Details[condition.Field],
				Description: fmt.Sprintf("Condition met: %s %s %v", condition.Field, condition.Operator, condition.Value),
				Severity:    te.determineThreatLevel(condition.Weight).String(),
				Confidence:  condition.Weight / 10.0,
			}
			indicators = append(indicators, indicator)
		}
	}
	
	return indicators
}

// processThreat processes a detected threat
func (te *ThreatEngine) processThreat(threat Threat) {
	te.activeThreats[threat.ID] = &threat
	te.threatHistory = append(te.threatHistory, threat)
	
	// Apply automatic mitigation actions
	for _, rule := range te.rules {
		if rule.Type == threat.Type {
			for _, action := range rule.Actions {
				if action.Enabled {
					te.executeAction(action, threat)
				}
			}
		}
	}
	
	// Log threat detection
	if te.logger != nil {
		te.logger.LogThreatDetection(threat)
	}
}

// executeAction executes automatic threat response action
func (te *ThreatEngine) executeAction(action ThreatResponseAction, threat Threat) {
	// Apply delay if specified
	if action.Delay > 0 {
		time.Sleep(action.Delay)
	}
	
	switch action.Type {
	case "block_ip":
		// Block malicious IP
		if ip, exists := action.Parameters["ip"]; exists {
			te.database.BlockIP(ip.(string))
		}
	case "block_user":
		// Block malicious user
		if userID, exists := action.Parameters["user_id"]; exists {
			te.database.BlockUser(userID.(string))
		}
	case "terminate_session":
		// Terminate user session
		if sessionID, exists := action.Parameters["session_id"]; exists {
			te.database.TerminateSession(sessionID.(string))
		}
	case "alert":
		// Send security alert
		if te.logger != nil {
			te.logger.LogThreatAlert(threat, action.Parameters)
		}
	}
}

// generateThreatID generates unique threat ID
func (te *ThreatEngine) generateThreatID() string {
	return fmt.Sprintf("threat_%d", time.Now().UnixNano())
}

// initializeDefaultRules initializes default threat detection rules
func (te *ThreatEngine) initializeDefaultRules() {
	defaultRules := []*ThreatDetectionRule{
		{
			ID:          "brute_force_detection",
			Name:        "Brute Force Attack Detection",
			Type:        ThreatTypeBruteForce,
			Description: "Detects brute force login attempts",
			Enabled:     true,
			Priority:    1,
			Conditions: []DetectionCondition{
				{
					Field:    "type",
					Operator: "equals",
					Value:    EventAuthFailure,
					Weight:   3.0,
				},
			},
			Actions: []ThreatResponseAction{
				{
					Type:        "alert",
					Enabled:     true,
					Description: "Send security alert",
				},
				{
					Type:        "block_ip",
					Enabled:     true,
					Delay:       5 * time.Minute,
					Description: "Block source IP after 5 minutes",
					Parameters: map[string]interface{}{
						"duration": 24 * time.Hour,
					},
				},
			},
			Threshold: DetectionThreshold{
				Count:      5,
				Score:      10.0,
				TimeWindow: 10 * time.Minute,
				Confidence: 0.8,
			},
			Window: 10 * time.Minute,
		},
		{
			ID:          "sql_injection_detection",
			Name:        "SQL Injection Attack Detection",
			Type:        ThreatTypeSQLInjection,
			Description: "Detects SQL injection attempts",
			Enabled:     true,
			Priority:    1,
			Conditions: []DetectionCondition{
				{
					Field:    "details.payload",
					Operator: "contains",
					Value:    "' OR '1'='1",
					Weight:   5.0,
				},
				{
					Field:    "details.payload",
					Operator: "regex",
					Value:    "(?i)(union|select|insert|update|delete)\\s+",
					Weight:   5.0,
				},
			},
			Actions: []ThreatResponseAction{
				{
					Type:        "alert",
					Enabled:     true,
					Description: "Send immediate security alert",
				},
				{
					Type:        "block_ip",
					Enabled:     true,
					Delay:       0,
					Description: "Immediately block source IP",
					Parameters: map[string]interface{}{
						"duration": 7 * 24 * time.Hour, // 7 days
					},
				},
			},
			Threshold: DetectionThreshold{
				Count:      1,
				Score:      5.0,
				TimeWindow:  1 * time.Minute,
				Confidence:  0.7,
			},
			Window: 1 * time.Minute,
		},
	}
	
	for _, rule := range defaultRules {
		te.rules[rule.ID] = rule
	}
}

// AnalyzeEvent analyzes event using ML model
func (mld *MLThreatDetector) AnalyzeEvent(event SecurityEvent) []Threat {
	if !mld.enabled {
		return nil
	}
	
	// Simplified ML detection
	// In production, use proper ML models
	var threats []Threat
	
	// Analyze event features
	score := mld.calculateThreatScore(event)
	if score >= mld.threshold {
		threat := Threat{
			ID:          mld.generateThreatID(),
			Type:        ThreatTypeAnomaly,
			Level:       mld.determineThreatLevel(score),
			Title:       "ML-detected anomaly",
			Description: "Machine learning model detected anomalous behavior",
			Timestamp:   time.Now(),
			UserID:      event.UserID,
			SessionID:   event.SessionID,
			Score:       score,
			Confidence:  0.8,
			Status:      ThreatStatusActive,
			Metadata: map[string]interface{}{
				"model_score": score,
				"threshold":   mld.threshold,
			},
		}
		threats = append(threats, threat)
	}
	
	return threats
}

// AnalyzeBehavior analyzes user behavior using ML
func (mld *MLThreatDetector) AnalyzeBehavior(userID string, actions []UserAction) []BehaviorAnomaly {
	if !mld.enabled {
		return nil
	}
	
	// Simplified behavior analysis
	// In production, use proper ML models
	var anomalies []BehaviorAnomaly
	
	// Check for unusual patterns
	if len(actions) > 100 && time.Since(actions[0].Timestamp) < 1*time.Hour {
		anomaly := BehaviorAnomaly{
			Type:        "high_frequency_actions",
			Description: "Unusually high frequency of actions",
			Pattern:     "rapid_succession",
			Score:       7.5,
			Confidence:  0.9,
		}
		anomalies = append(anomalies, anomaly)
	}
	
	return anomalies
}

// calculateThreatScore calculates threat score using ML model
func (mld *MLThreatDetector) calculateThreatScore(event SecurityEvent) float64 {
	// Simplified threat scoring
	// In production, use proper ML models
	score := 0.0
	
	switch event.Type {
	case EventAuthFailure:
		score += 3.0
	case EventNetworkBlocked:
		score += 2.0
	case EventFileBlocked:
		score += 4.0
	case EventSecurityViolation:
		score += 5.0
	case EventSuspiciousActivity:
		score += 3.5
	}
	
	// Add factors from event details
	if severity, exists := event.Details["risk_score"]; exists {
		if scoreFloat, ok := severity.(float64); ok {
			score += scoreFloat
		}
	}
	
	return score
}

// determineThreatLevel determines threat level from ML score
func (mld *MLThreatDetector) determineThreatLevel(score float64) ThreatLevel {
	if score >= 8.0 {
		return ThreatLevelCritical
	} else if score >= 6.0 {
		return ThreatLevelHigh
	} else if score >= 4.0 {
		return ThreatLevelMedium
	} else {
		return ThreatLevelLow
	}
}

// generateThreatID generates unique ML threat ID
func (mld *MLThreatDetector) generateThreatID() string {
	return fmt.Sprintf("ml_threat_%d", time.Now().UnixNano())
}

// initializeThreatIntelligence initializes threat database
func (td *ThreatDatabase) initializeThreatIntelligence() {
	// Initialize malicious IPs
	td.indicators["malicious_ips"] = []ThreatIndicator{
		{
			Type:        "malicious_ip",
			Value:       "192.168.1.100",
			Description: "Known malicious IP",
			Severity:    "high",
			Confidence:  0.9,
		},
	}
	
	// Initialize known attack signatures
	td.signatures["sql_injection"] = "' OR '1'='1"
	td.signatures["xss"] = "<script>alert('xss')</script>"
	
	// Initialize IOCs
	td.iocs["malicious_domains"] = []string{"evil-site.com", "malware-domain.net"}
	
	td.lastUpdate = time.Now()
}

// IsMaliciousIP checks if IP is in threat intelligence database
func (td *ThreatDatabase) IsMaliciousIP(ip string) bool {
	if indicators, exists := td.indicators["malicious_ips"]; exists {
		for _, indicator := range indicators {
			if indicator.Value == ip {
				return true
			}
		}
	}
	return false
}

// CheckEvent checks event against threat intelligence
func (td *ThreatDatabase) CheckEvent(event SecurityEvent) []Threat {
	var threats []Threat
	
	// Check against known signatures
	for _, value := range event.Details {
		if strVal, ok := value.(string); ok {
			for signatureType, signature := range td.signatures {
				if strings.Contains(strVal, signature) {
					threatType := ThreatTypeInjection
					if signatureType == "xss" {
						threatType = ThreatTypeXSS
					} else if signatureType == "sql_injection" {
						threatType = ThreatTypeSQLInjection
					}
					
					threat := Threat{
						ID:          td.generateThreatID(),
						Type:        threatType,
						Level:       ThreatLevelHigh,
						Title:       "Threat intelligence match",
						Description: fmt.Sprintf("Event matches known threat signature: %s", signature),
						Timestamp:   time.Now(),
						UserID:      event.UserID,
						SessionID:   event.SessionID,
						Score:       7.0,
						Confidence:  0.9,
						Status:      ThreatStatusActive,
						Indicators: []ThreatIndicator{
							{
								Type:        "threat_signature",
								Value:       signature,
								Description: "Known threat signature",
								Severity:    "high",
								Confidence:  0.9,
							},
						},
						Metadata: map[string]interface{}{
							"signature_type": signatureType,
							"signature":     signature,
						},
					}
					threats = append(threats, threat)
				}
			}
		}
	}
	
	return threats
}

// BlockIP adds IP to blocklist
func (td *ThreatDatabase) BlockIP(ip string) {
	if indicators, exists := td.indicators["blocked_ips"]; exists {
		indicator := ThreatIndicator{
			Type:        "blocked_ip",
			Value:       ip,
			Description: "IP blocked by security system",
			Severity:    "high",
			Confidence:  1.0,
		}
		td.indicators["blocked_ips"] = append(indicators, indicator)
	}
}

// BlockUser adds user to blocklist
func (td *ThreatDatabase) BlockUser(userID string) {
	if indicators, exists := td.indicators["blocked_users"]; exists {
		indicator := ThreatIndicator{
			Type:        "blocked_user",
			Value:       userID,
			Description: "User blocked by security system",
			Severity:    "high",
			Confidence:  1.0,
		}
		td.indicators["blocked_users"] = append(indicators, indicator)
	}
}

// TerminateSession terminates user session
func (td *ThreatDatabase) TerminateSession(sessionID string) {
	// In production, implement session termination
	// This is a placeholder
}

// generateThreatID generates unique threat ID
func (td *ThreatDatabase) generateThreatID() string {
	return fmt.Sprintf("ti_threat_%d", time.Now().UnixNano())
}

// String methods for enum types
func (tl ThreatLevel) String() string {
	return string(tl)
}

func (tt ThreatType) String() string {
	return string(tt)
}

func (ts ThreatStatus) String() string {
	return string(ts)
}

// LogThreatDetection logs threat detection events
func (sl *SecurityLogger) LogThreatDetection(threat Threat) {
	event := SecurityEvent{
		Type:        SecurityEventType("threat_detected"),
		Severity:    SecuritySeverity(threat.Level.String()),
		UserID:      threat.UserID,
		SessionID:   threat.SessionID,
		Description: fmt.Sprintf("Security threat detected: %s", threat.Title),
		Details: map[string]interface{}{
			"threat_id":    threat.ID,
			"threat_type":  string(threat.Type),
			"threat_level": string(threat.Level),
			"score":        threat.Score,
			"confidence":   threat.Confidence,
			"source_ip":    threat.SourceIP,
			"indicators":   threat.Indicators,
		},
	}
	
	sl.LogEvent(event)
}

// LogThreatMitigation logs threat mitigation events
func (sl *SecurityLogger) LogThreatMitigation(threatID, mitigation, userID, sessionID string) {
	event := SecurityEvent{
		Type:        SecurityEventType("threat_mitigated"),
		Severity:    SeverityInfo,
		UserID:      userID,
		SessionID:   sessionID,
		Description: fmt.Sprintf("Threat mitigation applied: %s", mitigation),
		Details: map[string]interface{}{
			"threat_id":   threatID,
			"mitigation":  mitigation,
		},
	}
	
	sl.LogEvent(event)
}

// LogThreatRuleAdd logs threat rule management events
func (sl *SecurityLogger) LogThreatRuleAdd(ruleID, ruleName, action string) {
	event := SecurityEvent{
		Type:        SecurityEventType("threat_rule_management"),
		Severity:    SeverityMedium,
		Description: fmt.Sprintf("Threat rule %s: %s", action, ruleName),
		Details: map[string]interface{}{
			"rule_id":   ruleID,
			"rule_name": ruleName,
			"action":    action,
		},
	}
	
	sl.LogEvent(event)
}

// LogThreatAlert logs threat alert events
func (sl *SecurityLogger) LogThreatAlert(threat Threat, parameters map[string]interface{}) {
	event := SecurityEvent{
		Type:        SecurityEventType("threat_alert"),
		Severity:    SecuritySeverity(threat.Level.String()),
		UserID:      threat.UserID,
		SessionID:   threat.SessionID,
		Description: fmt.Sprintf("SECURITY ALERT: %s", threat.Title),
		Details: map[string]interface{}{
			"threat_id":   threat.ID,
			"threat_type": string(threat.Type),
			"threat_level": string(threat.Level),
			"score":       threat.Score,
			"confidence":  threat.Confidence,
			"parameters":  parameters,
			"mitigation":  "automatic_response",
		},
	}
	
	if threat.Level == ThreatLevelCritical || threat.Level == ThreatLevelHigh {
		event.Severity = SeverityCritical
	}
	
	sl.LogEvent(event)
}