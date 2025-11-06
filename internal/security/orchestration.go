package security

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"
	"time"
)

// SecurityOrchestrator manages automated security responses and workflows
type SecurityOrchestrator struct {
	workflows     map[string]*SecurityWorkflow
	runningTasks  map[string]*OrchestrationTask
	threatEngine  *ThreatEngine
	logger        *SecurityLogger
	notificationManager *NotificationManager
	remediationManager *RemediationManager
	complianceManager *ComplianceManager
	mutex         sync.RWMutex
	ctx           context.Context
	cancel        context.CancelFunc
}

// SecurityWorkflow represents automated security response workflows
type SecurityWorkflow struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Description string                 `json:"description"`
	Triggers    []WorkflowTrigger      `json:"triggers"`
	Steps       []WorkflowStep         `json:"steps"`
	Enabled     bool                   `json:"enabled"`
	Priority    int                    `json:"priority"`
	Timeout     time.Duration          `json:"timeout"`
	CreatedAt   time.Time              `json:"created_at"`
	UpdatedAt   time.Time              `json:"updated_at"`
	Metadata    map[string]interface{} `json:"metadata"`
}

// WorkflowTrigger represents workflow activation conditions
type WorkflowTrigger struct {
	Type        string                 `json:"type"`
	Condition   string                 `json:"condition"`
	Parameters  map[string]interface{} `json:"parameters"`
	Enabled     bool                   `json:"enabled"`
}

// WorkflowStep represents individual workflow steps
type WorkflowStep struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Type        string                 `json:"type"`
	Action      string                 `json:"action"`
	Parameters  map[string]interface{} `json:"parameters"`
	Condition   string                 `json:"condition,omitempty"`
	Timeout     time.Duration          `json:"timeout"`
	RetryPolicy *RetryPolicy          `json:"retry_policy,omitempty"`
	OnError     string                 `json:"on_error,omitempty"`
}

// RetryPolicy defines retry behavior for workflow steps
type RetryPolicy struct {
	MaxAttempts int           `json:"max_attempts"`
	Delay       time.Duration `json:"delay"`
	Backoff     string        `json:"backoff"`
	MaxDelay    time.Duration `json:"max_delay"`
}

// OrchestrationTask represents a running workflow task
type OrchestrationTask struct {
	ID           string                 `json:"id"`
	WorkflowID   string                 `json:"workflow_id"`
	TriggerID    string                 `json:"trigger_id"`
	Status       TaskStatus              `json:"status"`
	StartedAt    time.Time              `json:"started_at"`
	CompletedAt  *time.Time             `json:"completed_at,omitempty"`
	Duration     time.Duration          `json:"duration"`
	CurrentStep  int                    `json:"current_step"`
	TotalSteps   int                    `json:"total_steps"`
	Progress     float64                `json:"progress"`
	Context      map[string]interface{} `json:"context"`
	Result       *TaskResult             `json:"result,omitempty"`
	Error        string                 `json:"error,omitempty"`
	Retries      int                    `json:"retries"`
	Metadata     map[string]interface{} `json:"metadata"`
}

// TaskStatus represents workflow task status
type TaskStatus string

const (
	TaskStatusPending     TaskStatus = "pending"
	TaskStatusRunning     TaskStatus = "running"
	TaskStatusCompleted   TaskStatus = "completed"
	TaskStatusFailed      TaskStatus = "failed"
	TaskStatusCancelled   TaskStatus = "cancelled"
	TaskStatusTimeout     TaskStatus = "timeout"
)

// TaskResult represents workflow task results
type TaskResult struct {
	Success    bool                   `json:"success"`
	Data       map[string]interface{} `json:"data"`
	Metrics    map[string]interface{} `json:"metrics"`
	Summary    string                 `json:"summary"`
	NextSteps  []string               `json:"next_steps"`
}

// NotificationManager handles security notifications and alerts
type NotificationManager struct {
	channels     map[string]*NotificationChannel
	templates    map[string]*NotificationTemplate
	rules        map[string]*NotificationRule
	subscriptions map[string][]string
	logger       *SecurityLogger
}

// NotificationChannel represents notification delivery channels
type NotificationChannel struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Type        string                 `json:"type"`
	Enabled     bool                   `json:"enabled"`
	Configuration map[string]interface{} `json:"configuration"`
	Settings    map[string]interface{} `json:"settings"`
	CreatedAt   time.Time              `json:"created_at"`
	UpdatedAt   time.Time              `json:"updated_at"`
}

// NotificationTemplate represents notification message templates
type NotificationTemplate struct {
	ID          string            `json:"id"`
	Name        string            `json:"name"`
	Type        string            `json:"type"`
	Subject     string            `json:"subject"`
	Body        string            `json:"body"`
	Format      string            `json:"format"`
	Variables   []string          `json:"variables"`
	CreatedAt   time.Time         `json:"created_at"`
	UpdatedAt   time.Time         `json:"updated_at"`
}

// NotificationRule defines when and how to send notifications
type NotificationRule struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Trigger     string                 `json:"trigger"`
	Condition   string                 `json:"condition"`
	Channels    []string               `json:"channels"`
	Template    string                 `json:"template"`
	Enabled     bool                   `json:"enabled"`
	Severity    []string               `json:"severity"`
	Throttle    NotificationThrottle     `json:"throttle"`
	CreatedAt   time.Time              `json:"created_at"`
	UpdatedAt   time.Time              `json:"updated_at"`
}

// NotificationThrottle controls notification frequency
type NotificationThrottle struct {
	Type      string        `json:"type"`
	Interval  time.Duration `json:"interval"`
	MaxCount  int           `json:"max_count"`
	Window    time.Duration `json:"window"`
}

// RemediationManager manages automated security remediation actions
type RemediationManager struct {
	playbooks   map[string]*RemediationPlaybook
	actions     map[string]*RemediationAction
	risks       map[string]*RiskAssessment
	logger      *SecurityLogger
}

// RemediationPlaybook represents automated security remediation procedures
type RemediationPlaybook struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Description string                 `json:"description"`
	ThreatType  ThreatType             `json:"threat_type"`
	ThreatLevel ThreatLevel            `json:"threat_level"`
	Steps       []RemediationStep      `json:"steps"`
	Actions     []RemediationAction     `json:"actions"`
	Conditions  []PlaybookCondition     `json:"conditions"`
	Enabled     bool                   `json:"enabled"`
	Priority    int                    `json:"priority"`
	AutoExecute bool                   `json:"auto_execute"`
	CreatedAt   time.Time              `json:"created_at"`
	UpdatedAt   time.Time              `json:"updated_at"`
}

// RemediationStep represents individual remediation steps
type RemediationStep struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Description string                 `json:"description"`
	Action      string                 `json:"action"`
	Parameters  map[string]interface{} `json:"parameters"`
	Order       int                    `json:"order"`
	Required    bool                   `json:"required"`
	Timeout     time.Duration          `json:"timeout"`
	RetryPolicy *RetryPolicy          `json:"retry_policy,omitempty"`
}

// RemediationAction represents automated remediation actions
type RemediationAction struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Type        string                 `json:"type"`
	Command     string                 `json:"command"`
	Parameters  map[string]interface{} `json:"parameters"`
	Conditions  []ActionCondition      `json:"conditions"`
	Enabled     bool                   `json:"enabled"`
	Critical    bool                   `json:"critical"`
	CreatedAt   time.Time              `json:"created_at"`
	UpdatedAt   time.Time              `json:"updated_at"`
}

// PlaybookCondition represents playbook execution conditions
type PlaybookCondition struct {
	Field    string      `json:"field"`
	Operator string      `json:"operator"`
	Value    interface{} `json:"value"`
	Required bool        `json:"required"`
}

// ActionCondition represents action execution conditions
type ActionCondition struct {
	Type        string      `json:"type"`
	Field       string      `json:"field"`
	Operator    string      `json:"operator"`
	Value       interface{} `json:"value"`
	Required    bool        `json:"required"`
	Description string      `json:"description"`
}

// RiskAssessment represents security risk assessments
type RiskAssessment struct {
	ID            string                 `json:"id"`
	Title         string                 `json:"title"`
	Description   string                 `json:"description"`
	RiskType      string                 `json:"risk_type"`
	RiskLevel     string                 `json:"risk_level"`
	Impact        string                 `json:"impact"`
	Likelihood    string                 `json:"likelihood"`
	Score         float64                `json:"score"`
	CVSS          float64                `json:"cvss"`
	CVE           []string               `json:"cve"`
	Assets        []string               `json:"assets"`
	Mitigations   []string               `json:"mitigations"`
	Timeline      AssessmentTimeline      `json:"timeline"`
	Recommendations []string            `json:"recommendations"`
	CreatedAt     time.Time              `json:"created_at"`
	UpdatedAt     time.Time              `json:"updated_at"`
	AssessedBy    string                 `json:"assessed_by"`
	ApprovedBy    string                 `json:"approved_by"`
	Status        AssessmentStatus        `json:"status"`
	Metadata      map[string]interface{} `json:"metadata"`
}

// AssessmentTimeline represents risk assessment timeline
type AssessmentTimeline struct {
	Identified   time.Time `json:"identified"`
	Assessed     time.Time `json:"assessed"`
	Mitigated     *time.Time `json:"mitigated,omitempty"`
	Resolved     *time.Time `json:"resolved,omitempty"`
	ReviewDue    time.Time `json:"review_due"`
	NextReview   *time.Time `json:"next_review,omitempty"`
}

// AssessmentStatus represents assessment status
type AssessmentStatus string

const (
	AssessmentStatusDraft       AssessmentStatus = "draft"
	AssessmentStatusPending    AssessmentStatus = "pending"
	AssessmentStatusApproved    AssessmentStatus = "approved"
	AssessmentStatusInProgress  AssessmentStatus = "in_progress"
	AssessmentStatusCompleted   AssessmentStatus = "completed"
	AssessmentStatusRejected    AssessmentStatus = "rejected"
)

// ComplianceManager manages regulatory compliance automation
type ComplianceManager struct {
	frameworks   map[string]*ComplianceFramework
	policies     map[string]*CompliancePolicy
	assessments  map[string]*ComplianceAssessment
	reports      map[string]*ComplianceReport
	logger       *SecurityLogger
}

// ComplianceFramework represents regulatory frameworks
type ComplianceFramework struct {
	ID            string                 `json:"id"`
	Name          string                 `json:"name"`
	Version       string                 `json:"version"`
	Description   string                 `json:"description"`
	Requirements  []ComplianceRequirement `json:"requirements"`
	Controls      []ComplianceControl    `json:"controls"`
	Mappings      []FrameworkMapping     `json:"mappings"`
	Enabled       bool                   `json:"enabled"`
	CreatedAt     time.Time              `json:"created_at"`
	UpdatedAt     time.Time              `json:"updated_at"`
}

// ComplianceRequirement represents compliance requirements
type ComplianceRequirement struct {
	ID          string                 `json:"id"`
	Title       string                 `json:"title"`
	Description string                 `json:"description"`
	Category    string                 `json:"category"`
	Priority    string                 `json:"priority"`
	Mandatory   bool                   `json:"mandatory"`
	Evidence    []EvidenceRequirement   `json:"evidence"`
	Controls    []string               `json:"controls"`
	Tests       []ComplianceTest       `json:"tests"`
}

// ComplianceControl represents security controls
type ComplianceControl struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Description string                 `json:"description"`
	Type        string                 `json:"type"`
	Category    string                 `json:"category"`
	Class       string                 `json:"class"`
	Implementation string              `json:"implementation"`
	Validation  []ControlValidation    `json:"validation"`
	Automated   bool                   `json:"automated"`
	Frequency   string                 `json:"frequency"`
	Responsible string                 `json:"responsible"`
}

// CompliancePolicy represents organizational compliance policies
type CompliancePolicy struct {
	ID          string                 `json:"id"`
	Name        string                 `json:"name"`
	Description string                 `json:"description"`
	Type        string                 `json:"type"`
	Framework   string                 `json:"framework"`
	Version     string                 `json:"version"`
	Status      PolicyStatus           `json:"status"`
	Effective   time.Time              `json:"effective"`
	Expires     *time.Time             `json:"expires,omitempty"`
	Requirements []string              `json:"requirements"`
	Controls    []string               `json:"controls"`
	Exceptions  []PolicyException      `json:"exceptions"`
	Approvals   []PolicyApproval       `json:"approvals"`
	ReviewDate  time.Time              `json:"review_date"`
	CreatedAt   time.Time              `json:"created_at"`
	UpdatedAt   time.Time              `json:"updated_at"`
	Owner       string                 `json:"owner"`
}

// NewSecurityOrchestrator creates a new security orchestrator
func NewSecurityOrchestrator(logger *SecurityLogger) *SecurityOrchestrator {
	ctx, cancel := context.WithCancel(context.Background())
	
	return &SecurityOrchestrator{
		workflows:            make(map[string]*SecurityWorkflow),
		runningTasks:         make(map[string]*OrchestrationTask),
		logger:              logger,
		notificationManager:  NewNotificationManager(logger),
		remediationManager:  NewRemediationManager(logger),
		complianceManager:   NewComplianceManager(logger),
		ctx:                 ctx,
		cancel:              cancel,
	}
}

// Start starts the security orchestrator
func (so *SecurityOrchestrator) Start() error {
	// Initialize default workflows
	so.initializeDefaultWorkflows()
	
	// Start background processes
	go so.monitorThreats()
	go so.cleanupCompletedTasks()
	
	// Log orchestrator start
	if so.logger != nil {
		so.logger.LogOrchestrationEvent("orchestrator_started", "Security orchestrator started successfully", nil)
	}
	
	return nil
}

// Stop stops the security orchestrator
func (so *SecurityOrchestrator) Stop() error {
	so.cancel()
	
	// Cancel all running tasks
	so.mutex.Lock()
	for _, task := range so.runningTasks {
		task.Status = TaskStatusCancelled
	}
	so.mutex.Unlock()
	
	// Log orchestrator stop
	if so.logger != nil {
		so.logger.LogOrchestrationEvent("orchestrator_stopped", "Security orchestrator stopped", nil)
	}
	
	return nil
}

// AddWorkflow adds a new security workflow
func (so *SecurityOrchestrator) AddWorkflow(workflow *SecurityWorkflow) error {
	so.mutex.Lock()
	defer so.mutex.Unlock()
	
	workflow.CreatedAt = time.Now()
	workflow.UpdatedAt = time.Now()
	workflow.TotalSteps = len(workflow.Steps)
	
	so.workflows[workflow.ID] = workflow
	
	// Log workflow addition
	if so.logger != nil {
		so.logger.LogOrchestrationEvent("workflow_added", fmt.Sprintf("Workflow added: %s", workflow.Name), map[string]interface{}{
			"workflow_id":   workflow.ID,
			"workflow_name": workflow.Name,
			"triggers":     workflow.Triggers,
		})
	}
	
	return nil
}

// ExecuteWorkflow executes a security workflow
func (so *SecurityOrchestrator) ExecuteWorkflow(workflowID string, triggerData map[string]interface{}, ctx *Context) (*OrchestrationTask, error) {
	so.mutex.Lock()
	defer so.mutex.Unlock()
	
	workflow, exists := so.workflows[workflowID]
	if !exists {
		return nil, fmt.Errorf("workflow not found: %s", workflowID)
	}
	
	if !workflow.Enabled {
		return nil, fmt.Errorf("workflow is disabled: %s", workflowID)
	}
	
	// Create orchestration task
	task := &OrchestrationTask{
		ID:          so.generateTaskID(),
		WorkflowID:  workflowID,
		Status:      TaskStatusPending,
		StartedAt:   time.Now(),
		CurrentStep: 0,
		TotalSteps:   len(workflow.Steps),
		Progress:    0.0,
		Context:     triggerData,
		Metadata: map[string]interface{}{
			"triggered_by": ctx.UserID,
			"session_id":   ctx.SessionID,
		},
	}
	
	so.runningTasks[task.ID] = task
	
	// Execute workflow asynchronously
	go so.executeWorkflow(task, workflow, ctx)
	
	return task, nil
}

// executeWorkflow executes a workflow step by step
func (so *SecurityOrchestrator) executeWorkflow(task *OrchestrationTask, workflow *SecurityWorkflow, ctx *Context) {
	task.Status = TaskStatusRunning
	
	// Set timeout
	timeout := time.After(workflow.Timeout)
	
	for i, step := range workflow.Steps {
		// Check if cancelled
		select {
		case <-so.ctx.Done():
			task.Status = TaskStatusCancelled
			return
		case <-timeout:
			task.Status = TaskStatusTimeout
			task.Error = "Workflow execution timeout"
			return
		default:
		}
		
		task.CurrentStep = i + 1
		task.Progress = float64(i+1) / float64(len(workflow.Steps)) * 100
		
		// Execute step
		stepResult, err := so.executeStep(step, task, ctx)
		if err != nil {
			task.Error = err.Error()
			
			// Handle error based on onError policy
			if step.OnError == "continue" {
				continue
			} else if step.OnError == "retry" && step.RetryPolicy != nil {
				// Retry step
				if err := so.retryStep(step, task, ctx); err != nil {
					task.Status = TaskStatusFailed
					return
				}
			} else {
				task.Status = TaskStatusFailed
				return
			}
		}
		
		// Store step result
		if task.Context == nil {
			task.Context = make(map[string]interface{})
		}
		task.Context[fmt.Sprintf("step_%d_result", i+1)] = stepResult
	}
	
	// Workflow completed successfully
	task.Status = TaskStatusCompleted
	now := time.Now()
	task.CompletedAt = &now
	task.Duration = now.Sub(task.StartedAt)
	
	// Log workflow completion
	if so.logger != nil {
		so.logger.LogOrchestrationEvent("workflow_completed", fmt.Sprintf("Workflow completed: %s", workflow.Name), map[string]interface{}{
			"task_id":     task.ID,
			"workflow_id": workflow.ID,
			"duration":    task.Duration,
			"steps":       task.TotalSteps,
		})
	}
}

// executeStep executes an individual workflow step
func (so *SecurityOrchestrator) executeStep(step WorkflowStep, task *OrchestrationTask, ctx *Context) (map[string]interface{}, error) {
	// Create step context
	stepCtx := map[string]interface{}{
		"step_id":     step.ID,
		"step_name":   step.Name,
		"step_type":   step.Type,
		"task_id":     task.ID,
		"workflow_id": task.WorkflowID,
		"task_context": task.Context,
		"parameters":  step.Parameters,
	}
	
	// Execute step based on type
	switch step.Type {
	case "notification":
		return so.executeNotificationStep(step, stepCtx)
	case "remediation":
		return so.executeRemediationStep(step, stepCtx)
	case "assessment":
		return so.executeAssessmentStep(step, stepCtx)
	case "compliance":
		return so.executeComplianceStep(step, stepCtx)
	case "script":
		return so.executeScriptStep(step, stepCtx)
	case "api_call":
		return so.executeAPICallStep(step, stepCtx)
	default:
		return nil, fmt.Errorf("unknown step type: %s", step.Type)
	}
}

// executeNotificationStep executes notification workflow step
func (so *SecurityOrchestrator) executeNotificationStep(step WorkflowStep, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Extract notification parameters
	channelID, exists := step.Parameters["channel_id"]
	if !exists {
		return nil, fmt.Errorf("channel_id parameter required for notification step")
	}
	
	message, exists := step.Parameters["message"]
	if !exists {
		return nil, fmt.Errorf("message parameter required for notification step")
	}
	
	severity, _ := step.Parameters["severity"].(string)
	if severity == "" {
		severity = "medium"
	}
	
	// Send notification through notification manager
	err := so.notificationManager.SendNotification(channelID.(string), message.(string), severity, ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to send notification: %w", err)
	}
	
	result := map[string]interface{}{
		"channel_id": channelID,
		"message":    message,
		"sent_at":    time.Now(),
	}
	
	return result, nil
}

// executeRemediationStep executes remediation workflow step
func (so *SecurityOrchestrator) executeRemediationStep(step WorkflowStep, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Extract remediation parameters
	playbookID, exists := step.Parameters["playbook_id"]
	if !exists {
		return nil, fmt.Errorf("playbook_id parameter required for remediation step")
	}
	
	threatID, _ := step.Parameters["threat_id"].(string)
	
	// Execute remediation playbook
	result, err := so.remediationManager.ExecutePlaybook(playbookID.(string), threatID, ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to execute remediation: %w", err)
	}
	
	return result, nil
}

// executeAssessmentStep executes assessment workflow step
func (so *SecurityOrchestrator) executeAssessmentStep(step WorkflowStep, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Extract assessment parameters
	riskType, _ := step.Parameters["risk_type"].(string)
	assetID, _ := step.Parameters["asset_id"].(string)
	
	// Perform risk assessment
	assessment, err := so.remediationManager.PerformRiskAssessment(riskType, assetID, ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to perform assessment: %w", err)
	}
	
	result := map[string]interface{}{
		"assessment_id": assessment.ID,
		"risk_score":    assessment.Score,
		"risk_level":    assessment.RiskLevel,
		"impact":        assessment.Impact,
		"likelihood":    assessment.Likelihood,
		"assessed_at":   time.Now(),
	}
	
	return result, nil
}

// executeComplianceStep executes compliance workflow step
func (so *SecurityOrchestrator) executeComplianceStep(step WorkflowStep, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Extract compliance parameters
	frameworkID, _ := step.Parameters["framework_id"].(string)
	policyID, _ := step.Parameters["policy_id"].(string)
	
	// Perform compliance check
	result, err := so.complianceManager.CheckCompliance(frameworkID, policyID, ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to perform compliance check: %w", err)
	}
	
	return result, nil
}

// executeScriptStep executes script workflow step
func (so *SecurityOrchestrator) executeScriptStep(step WorkflowStep, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Extract script parameters
	scriptPath, exists := step.Parameters["script_path"]
	if !exists {
		return nil, fmt.Errorf("script_path parameter required for script step")
	}
	
	scriptArgs, _ := step.Parameters["arguments"].([]interface{})
	
	// Execute script (placeholder implementation)
	result := map[string]interface{}{
		"script_path": scriptPath,
		"arguments":   scriptArgs,
		"executed_at": time.Now(),
		"output":      "Script execution result",
		"exit_code":   0,
	}
	
	return result, nil
}

// executeAPICallStep executes API call workflow step
func (so *SecurityOrchestrator) executeAPICallStep(step WorkflowStep, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Extract API parameters
	apiURL, exists := step.Parameters["url"]
	if !exists {
		return nil, fmt.Errorf("url parameter required for API call step")
	}
	
	method, _ := step.Parameters["method"].(string)
	if method == "" {
		method = "GET"
	}
	
	headers, _ := step.Parameters["headers"].(map[string]interface{})
	body, _ := step.Parameters["body"].(string)
	
	// Execute API call (placeholder implementation)
	result := map[string]interface{}{
		"url":         apiURL,
		"method":      method,
		"headers":     headers,
		"body":        body,
		"called_at":   time.Now(),
		"status_code":  200,
		"response":    "API response",
	}
	
	return result, nil
}

// retryStep implements step retry logic
func (so *SecurityOrchestrator) retryStep(step WorkflowStep, task *OrchestrationTask, ctx *Context) error {
	if step.RetryPolicy == nil {
		return fmt.Errorf("no retry policy specified")
	}
	
	policy := step.RetryPolicy
	delay := policy.Delay
	
	for attempt := 1; attempt <= policy.MaxAttempts; attempt++ {
		// Wait before retry
		time.Sleep(delay)
		
		// Execute step
		_, err := so.executeStep(step, task.Context, ctx)
		if err == nil {
			return nil // Success
		}
		
		// Update retry count
		task.Retries++
		
		// Apply backoff
		if policy.Backoff == "exponential" {
			delay *= 2
			if delay > policy.MaxDelay {
				delay = policy.MaxDelay
			}
		}
	}
	
	return fmt.Errorf("step retry failed after %d attempts", policy.MaxAttempts)
}

// GetTaskStatus returns status of a workflow task
func (so *SecurityOrchestrator) GetTaskStatus(taskID string) (*OrchestrationTask, error) {
	so.mutex.RLock()
	defer so.mutex.RUnlock()
	
	task, exists := so.runningTasks[taskID]
	if !exists {
		return nil, fmt.Errorf("task not found: %s", taskID)
	}
	
	return task, nil
}

// GetActiveTasks returns all active workflow tasks
func (so *SecurityOrchestrator) GetActiveTasks() []*OrchestrationTask {
	so.mutex.RLock()
	defer so.mutex.RUnlock()
	
	var activeTasks []*OrchestrationTask
	for _, task := range so.runningTasks {
		if task.Status == TaskStatusRunning || task.Status == TaskStatusPending {
			activeTasks = append(activeTasks, task)
		}
	}
	
	return activeTasks
}

// CancelTask cancels a running workflow task
func (so *SecurityOrchestrator) CancelTask(taskID string, reason string) error {
	so.mutex.Lock()
	defer so.mutex.Unlock()
	
	task, exists := so.runningTasks[taskID]
	if !exists {
		return fmt.Errorf("task not found: %s", taskID)
	}
	
	task.Status = TaskStatusCancelled
	task.Error = reason
	
	// Log task cancellation
	if so.logger != nil {
		so.logger.LogOrchestrationEvent("task_cancelled", fmt.Sprintf("Task cancelled: %s", taskID), map[string]interface{}{
			"task_id": taskID,
			"reason":   reason,
		})
	}
	
	return nil
}

// monitorThreats monitors for threats and triggers workflows
func (so *SecurityOrchestrator) monitorThreats() {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()
	
	for {
		select {
		case <-so.ctx.Done():
			return
		case <-ticker.C:
			// Check for new threats and trigger workflows
			if so.threatEngine != nil {
				threats := so.threatEngine.GetActiveThreats()
				for _, threat := range threats {
					so.processThreat(threat)
				}
			}
		}
	}
}

// processThreat processes a detected threat
func (so *SecurityOrchestrator) processThreat(threat Threat) {
	// Check if threat matches any workflow triggers
	so.mutex.RLock()
	defer so.mutex.RUnlock()
	
	for _, workflow := range so.workflows {
		if !workflow.Enabled {
			continue
		}
		
		for _, trigger := range workflow.Triggers {
			if so.matchesTrigger(trigger, threat) {
				// Execute workflow
				triggerData := map[string]interface{}{
					"threat_id":   threat.ID,
					"threat_type": threat.Type,
					"threat_level": threat.Level,
					"threat":      threat,
				}
				
				ctx := &Context{
					UserID:    threat.UserID,
					SessionID: threat.SessionID,
				}
				
				so.ExecuteWorkflow(workflow.ID, triggerData, ctx)
				break
			}
		}
	}
}

// matchesTrigger checks if threat matches workflow trigger
func (so *SecurityOrchestrator) matchesTrigger(trigger WorkflowTrigger, threat Threat) bool {
	switch trigger.Type {
	case "threat_type":
		return string(threat.Type) == trigger.Condition
	case "threat_level":
		return string(threat.Level) == trigger.Condition
	case "threat_score":
		if minScore, ok := trigger.Parameters["min_score"].(float64); ok {
			return threat.Score >= minScore
		}
	case "custom":
		// Implement custom trigger logic
		return false
	default:
		return false
	}
	
	return false
}

// cleanupCompletedTasks removes old completed tasks
func (so *SecurityOrchestrator) cleanupCompletedTasks() {
	ticker := time.NewTicker(1 * time.Hour)
	defer ticker.Stop()
	
	for {
		select {
		case <-so.ctx.Done():
			return
		case <-ticker.C:
			so.mutex.Lock()
			
			// Remove tasks completed more than 24 hours ago
			cutoff := time.Now().Add(-24 * time.Hour)
			for taskID, task := range so.runningTasks {
				if task.Status == TaskStatusCompleted || task.Status == TaskStatusFailed || task.Status == TaskStatusCancelled {
					if task.CompletedAt != nil && task.CompletedAt.Before(cutoff) {
						delete(so.runningTasks, taskID)
					}
				}
			}
			
			so.mutex.Unlock()
		}
	}
}

// generateTaskID generates unique task ID
func (so *SecurityOrchestrator) generateTaskID() string {
	return fmt.Sprintf("task_%d", time.Now().UnixNano())
}

// initializeDefaultWorkflows initializes default security workflows
func (so *SecurityOrchestrator) initializeDefaultWorkflows() {
	defaultWorkflows := []*SecurityWorkflow{
		{
			ID:          "critical_threat_response",
			Name:        "Critical Threat Response",
			Description: "Automated response to critical security threats",
			Enabled:     true,
			Priority:    1,
			Timeout:     30 * time.Minute,
			Triggers: []WorkflowTrigger{
				{
					Type:      "threat_level",
					Condition: "critical",
					Enabled:   true,
				},
			},
			Steps: []WorkflowStep{
				{
					ID:     "notify_security_team",
					Name:   "Notify Security Team",
					Type:   "notification",
					Action: "send_notification",
					Parameters: map[string]interface{}{
						"channel_id": "security_team",
						"message":   "CRITICAL: {{.threat.Title}} detected with score {{.threat.Score}}",
						"severity":  "critical",
					},
					Timeout: 1 * time.Minute,
				},
				{
					ID:     "remediate_threat",
					Name:   "Remediate Threat",
					Type:   "remediation",
					Action: "execute_playbook",
					Parameters: map[string]interface{}{
						"playbook_id": "critical_threat_remediation",
						"threat_id":   "{{.threat_id}}",
					},
					Timeout: 10 * time.Minute,
					RetryPolicy: &RetryPolicy{
						MaxAttempts: 3,
						Delay:       30 * time.Second,
						Backoff:     "exponential",
						MaxDelay:    5 * time.Minute,
					},
				},
				{
					ID:     "assess_impact",
					Name:   "Assess Impact",
					Type:   "assessment",
					Action: "perform_assessment",
					Parameters: map[string]interface{}{
						"risk_type": "security_incident",
						"threat_id": "{{.threat_id}}",
					},
					Timeout: 5 * time.Minute,
				},
			},
		},
		{
			ID:          "compliance_violation_response",
			Name:        "Compliance Violation Response",
			Description: "Automated response to compliance violations",
			Enabled:     true,
			Priority:    2,
			Timeout:     60 * time.Minute,
			Triggers: []WorkflowTrigger{
				{
					Type:      "custom",
					Condition: "compliance_failure",
					Enabled:   true,
				},
			},
			Steps: []WorkflowStep{
				{
					ID:     "notify_compliance_team",
					Name:   "Notify Compliance Team",
					Type:   "notification",
					Action: "send_notification",
					Parameters: map[string]interface{}{
						"channel_id": "compliance_team",
						"message":   "Compliance violation detected: {{.violation_type}}",
						"severity":  "high",
					},
					Timeout: 1 * time.Minute,
				},
				{
					ID:     "initiate_investigation",
					Name:   "Initiate Investigation",
					Type:   "compliance",
					Action: "start_investigation",
					Parameters: map[string]interface{}{
						"framework_id": "{{.framework_id}}",
						"policy_id":    "{{.policy_id}}",
					},
					Timeout: 5 * time.Minute,
				},
			},
		},
	}
	
	for _, workflow := range defaultWorkflows {
		so.workflows[workflow.ID] = workflow
	}
}

// NewNotificationManager creates a new notification manager
func NewNotificationManager(logger *SecurityLogger) *NotificationManager {
	return &NotificationManager{
		channels:     make(map[string]*NotificationChannel),
		templates:    make(map[string]*NotificationTemplate),
		rules:        make(map[string]*NotificationRule),
		subscriptions: make(map[string][]string),
		logger:       logger,
	}
}

// SendNotification sends a notification through specified channel
func (nm *NotificationManager) SendNotification(channelID, message, severity string, ctx map[string]interface{}) error {
	// Get channel
	channel, exists := nm.channels[channelID]
	if !exists || !channel.Enabled {
		return fmt.Errorf("channel not found or disabled: %s", channelID)
	}
	
	// Apply notification rules
	if !nm.shouldSendNotification(channelID, severity, ctx) {
		return nil // Throttled or not allowed
	}
	
	// Send notification based on channel type
	switch channel.Type {
	case "email":
		return nm.sendEmailNotification(channel, message, severity, ctx)
	case "slack":
		return nm.sendSlackNotification(channel, message, severity, ctx)
	case "webhook":
		return nm.sendWebhookNotification(channel, message, severity, ctx)
	case "sms":
		return nm.sendSMSNotification(channel, message, severity, ctx)
	default:
		return fmt.Errorf("unsupported channel type: %s", channel.Type)
	}
}

// sendEmailNotification sends email notification
func (nm *NotificationManager) sendEmailNotification(channel *NotificationChannel, message, severity string, ctx map[string]interface{}) error {
	// Implement email sending logic
	if nm.logger != nil {
		nm.logger.LogOrchestrationEvent("notification_sent", "Email notification sent", map[string]interface{}{
			"channel_type": "email",
			"message":      message,
			"severity":     severity,
		})
	}
	return nil
}

// sendSlackNotification sends Slack notification
func (nm *NotificationManager) sendSlackNotification(channel *NotificationChannel, message, severity string, ctx map[string]interface{}) error {
	// Implement Slack notification logic
	if nm.logger != nil {
		nm.logger.LogOrchestrationEvent("notification_sent", "Slack notification sent", map[string]interface{}{
			"channel_type": "slack",
			"message":      message,
			"severity":     severity,
		})
	}
	return nil
}

// sendWebhookNotification sends webhook notification
func (nm *NotificationManager) sendWebhookNotification(channel *NotificationChannel, message, severity string, ctx map[string]interface{}) error {
	// Implement webhook notification logic
	if nm.logger != nil {
		nm.logger.LogOrchestrationEvent("notification_sent", "Webhook notification sent", map[string]interface{}{
			"channel_type": "webhook",
			"message":      message,
			"severity":     severity,
		})
	}
	return nil
}

// sendSMSNotification sends SMS notification
func (nm *NotificationManager) sendSMSNotification(channel *NotificationChannel, message, severity string, ctx map[string]interface{}) error {
	// Implement SMS notification logic
	if nm.logger != nil {
		nm.logger.LogOrchestrationEvent("notification_sent", "SMS notification sent", map[string]interface{}{
			"channel_type": "sms",
			"message":      message,
			"severity":     severity,
		})
	}
	return nil
}

// shouldSendNotification checks if notification should be sent based on rules
func (nm *NotificationManager) shouldSendNotification(channelID, severity string, ctx map[string]interface{}) bool {
	// Check throttling rules
	for _, rule := range nm.rules {
		if rule.Enabled && nm.matchesNotificationRule(rule, channelID, severity, ctx) {
			// Check throttle
			if !nm.checkThrottle(rule, channelID) {
				return false // Throttled
			}
		}
	}
	
	return true
}

// matchesNotificationRule checks if notification matches rule
func (nm *NotificationManager) matchesNotificationRule(rule *NotificationRule, channelID, severity string, ctx map[string]interface{}) bool {
	// Check channel
	for _, ch := range rule.Channels {
		if ch == channelID {
			// Check severity
			for _, sev := range rule.Severity {
				if sev == severity {
					return true
				}
			}
		}
	}
	
	return false
}

// checkThrottle checks notification throttling
func (nm *NotificationManager) checkThrottle(rule *NotificationRule, channelID string) bool {
	// Implement throttling logic
	// This is a placeholder - implement proper rate limiting
	return true
}

// NewRemediationManager creates a new remediation manager
func NewRemediationManager(logger *SecurityLogger) *RemediationManager {
	return &RemediationManager{
		playbooks: make(map[string]*RemediationPlaybook),
		actions:   make(map[string]*RemediationAction),
		risks:     make(map[string]*RiskAssessment),
		logger:    logger,
	}
}

// ExecutePlaybook executes a remediation playbook
func (rm *RemediationManager) ExecutePlaybook(playbookID, threatID string, ctx map[string]interface{}) (map[string]interface{}, error) {
	playbook, exists := rm.playbooks[playbookID]
	if !exists {
		return nil, fmt.Errorf("playbook not found: %s", playbookID)
	}
	
	if !playbook.Enabled {
		return nil, fmt.Errorf("playbook is disabled: %s", playbookID)
	}
	
	// Execute remediation steps
	results := make(map[string]interface{})
	for _, step := range playbook.Steps {
		stepResult, err := rm.executeRemediationStep(step, threatID, ctx)
		if err != nil {
			if step.Required {
				return nil, fmt.Errorf("required remediation step failed: %w", err)
			}
			// Non-required step failed, continue
			continue
		}
		
		results[step.ID] = stepResult
	}
	
	return results, nil
}

// executeRemediationStep executes a remediation step
func (rm *RemediationManager) executeRemediationStep(step RemediationStep, threatID string, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Implement step execution based on action type
	switch step.Action {
	case "block_ip":
		return rm.executeBlockIPAction(step, threatID, ctx)
	case "terminate_session":
		return rm.executeTerminateSessionAction(step, threatID, ctx)
	case "disable_user":
		return rm.executeDisableUserAction(step, threatID, ctx)
	case "quarantine_asset":
		return rm.executeQuarantineAssetAction(step, threatID, ctx)
	default:
		return nil, fmt.Errorf("unknown remediation action: %s", step.Action)
	}
}

// executeBlockIPAction executes IP blocking action
func (rm *RemediationManager) executeBlockIPAction(step RemediationStep, threatID string, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Implement IP blocking logic
	result := map[string]interface{}{
		"action":     "block_ip",
		"threat_id":  threatID,
		"executed_at": time.Now(),
		"status":     "completed",
	}
	
	// Log action
	if rm.logger != nil {
		rm.logger.LogOrchestrationEvent("remediation_action", "IP blocked", result)
	}
	
	return result, nil
}

// executeTerminateSessionAction executes session termination action
func (rm *RemediationManager) executeTerminateSessionAction(step RemediationStep, threatID string, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Implement session termination logic
	result := map[string]interface{}{
		"action":     "terminate_session",
		"threat_id":  threatID,
		"executed_at": time.Now(),
		"status":     "completed",
	}
	
	// Log action
	if rm.logger != nil {
		rm.logger.LogOrchestrationEvent("remediation_action", "Session terminated", result)
	}
	
	return result, nil
}

// executeDisableUserAction executes user disabling action
func (rm *RemediationManager) executeDisableUserAction(step RemediationStep, threatID string, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Implement user disabling logic
	result := map[string]interface{}{
		"action":     "disable_user",
		"threat_id":  threatID,
		"executed_at": time.Now(),
		"status":     "completed",
	}
	
	// Log action
	if rm.logger != nil {
		rm.logger.LogOrchestrationEvent("remediation_action", "User disabled", result)
	}
	
	return result, nil
}

// executeQuarantineAssetAction executes asset quarantine action
func (rm *RemediationManager) executeQuarantineAssetAction(step RemediationStep, threatID string, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Implement asset quarantine logic
	result := map[string]interface{}{
		"action":     "quarantine_asset",
		"threat_id":  threatID,
		"executed_at": time.Now(),
		"status":     "completed",
	}
	
	// Log action
	if rm.logger != nil {
		rm.logger.LogOrchestrationEvent("remediation_action", "Asset quarantined", result)
	}
	
	return result, nil
}

// PerformRiskAssessment performs a risk assessment
func (rm *RemediationManager) PerformRiskAssessment(riskType, assetID string, ctx map[string]interface{}) (*RiskAssessment, error) {
	assessment := &RiskAssessment{
		ID:          rm.generateAssessmentID(),
		Title:       fmt.Sprintf("Risk Assessment: %s - %s", riskType, assetID),
		Description: fmt.Sprintf("Automated risk assessment for %s on asset %s", riskType, assetID),
		RiskType:    riskType,
		Assets:      []string{assetID},
		Timeline: AssessmentTimeline{
			Identified: time.Now(),
			Assessed:   time.Now(),
			ReviewDue:  time.Now().Add(30 * 24 * time.Hour), // 30 days
		},
		CreatedAt:   time.Now(),
		UpdatedAt:   time.Now(),
		Status:      AssessmentStatusCompleted,
		AssessedBy:  "security_system",
		Metadata:    ctx,
	}
	
	// Calculate risk score (simplified)
	score := rm.calculateRiskScore(riskType, assetID)
	assessment.Score = score
	assessment.RiskLevel = rm.determineRiskLevel(score)
	
	// Store assessment
	rm.risks[assessment.ID] = assessment
	
	// Log assessment
	if rm.logger != nil {
		rm.logger.LogOrchestrationEvent("risk_assessment", "Risk assessment completed", map[string]interface{}{
			"assessment_id": assessment.ID,
			"risk_type":    riskType,
			"asset_id":     assetID,
			"risk_score":   score,
			"risk_level":   assessment.RiskLevel,
		})
	}
	
	return assessment, nil
}

// calculateRiskScore calculates risk score
func (rm *RemediationManager) calculateRiskScore(riskType, assetID string) float64 {
	// Simplified risk scoring
	// In production, use proper risk assessment models
	baseScore := 5.0
	
	// Adjust based on risk type
	switch riskType {
	case "security_incident":
		baseScore += 3.0
	case "vulnerability":
		baseScore += 2.0
	case "compliance_violation":
		baseScore += 1.5
	}
	
	// Adjust based on asset (simplified)
	if assetID == "production_database" {
		baseScore += 4.0
	} else if assetID == "api_gateway" {
		baseScore += 2.5
	}
	
	return baseScore
}

// determineRiskLevel determines risk level from score
func (rm *RemediationManager) determineRiskLevel(score float64) string {
	if score >= 8.0 {
		return "critical"
	} else if score >= 6.0 {
		return "high"
	} else if score >= 4.0 {
		return "medium"
	} else if score >= 2.0 {
		return "low"
	} else {
		return "minimal"
	}
}

// generateAssessmentID generates unique assessment ID
func (rm *RemediationManager) generateAssessmentID() string {
	return fmt.Sprintf("assessment_%d", time.Now().UnixNano())
}

// NewComplianceManager creates a new compliance manager
func NewComplianceManager(logger *SecurityLogger) *ComplianceManager {
	return &ComplianceManager{
		frameworks:  make(map[string]*ComplianceFramework),
		policies:    make(map[string]*CompliancePolicy),
		assessments: make(map[string]*ComplianceAssessment),
		reports:     make(map[string]*ComplianceReport),
		logger:      logger,
	}
}

// CheckCompliance checks compliance against framework and policy
func (cm *ComplianceManager) CheckCompliance(frameworkID, policyID string, ctx map[string]interface{}) (map[string]interface{}, error) {
	// Get framework and policy
	framework, exists := cm.frameworks[frameworkID]
	if !exists {
		return nil, fmt.Errorf("framework not found: %s", frameworkID)
	}
	
	policy, exists := cm.policies[policyID]
	if !exists {
		return nil, fmt.Errorf("policy not found: %s", policyID)
	}
	
	// Perform compliance check
	results := make(map[string]interface{})
	
	// Check policy requirements
	complianceScore := 0.0
	totalRequirements := len(policy.Requirements)
	
	for _, reqID := range policy.Requirements {
		// Find requirement
		var requirement *ComplianceRequirement
		for _, req := range framework.Requirements {
			if req.ID == reqID {
				requirement = &req
				break
			}
		}
		
		if requirement == nil {
			continue
		}
		
		// Check requirement (simplified)
		compliant := cm.checkRequirement(requirement, ctx)
		if compliant {
			complianceScore += 1.0
		}
		
		results[reqID] = map[string]interface{}{
			"compliant": compliant,
			"mandatory": requirement.Mandatory,
			"category":  requirement.Category,
		}
	}
	
	// Calculate final compliance score
	if totalRequirements > 0 {
		complianceScore = (complianceScore / float64(totalRequirements)) * 100
	}
	
	results["compliance_score"] = complianceScore
	results["framework_id"] = frameworkID
	results["policy_id"] = policyID
	results["checked_at"] = time.Now()
	
	return results, nil
}

// checkRequirement checks individual compliance requirement
func (cm *ComplianceManager) checkRequirement(requirement *ComplianceRequirement, ctx map[string]interface{}) bool {
	// Simplified requirement checking
	// In production, implement proper compliance validation
	switch requirement.Category {
	case "access_control":
		return cm.checkAccessControl(requirement, ctx)
	case "encryption":
		return cm.checkEncryption(requirement, ctx)
	case "audit_logging":
		return cm.checkAuditLogging(requirement, ctx)
	case "data_protection":
		return cm.checkDataProtection(requirement, ctx)
	default:
		return true // Default to compliant for unknown categories
	}
}

// checkAccessControl checks access control requirements
func (cm *ComplianceManager) checkAccessControl(requirement *ComplianceRequirement, ctx map[string]interface{}) bool {
	// Implement access control checks
	return true
}

// checkEncryption checks encryption requirements
func (cm *ComplianceManager) checkEncryption(requirement *ComplianceRequirement, ctx map[string]interface{}) bool {
	// Implement encryption checks
	return true
}

// checkAuditLogging checks audit logging requirements
func (cm *ComplianceManager) checkAuditLogging(requirement *ComplianceRequirement, ctx map[string]interface{}) bool {
	// Implement audit logging checks
	return true
}

// checkDataProtection checks data protection requirements
func (cm *ComplianceManager) checkDataProtection(requirement *ComplianceRequirement, ctx map[string]interface{}) bool {
	// Implement data protection checks
	return true
}

// LogOrchestrationEvent logs security orchestration events
func (sl *SecurityLogger) LogOrchestrationEvent(eventType, description string, details map[string]interface{}) {
	event := SecurityEvent{
		Type:        SecurityEventType("orchestration"),
		Severity:    SeverityMedium,
		Description: description,
		Details: map[string]interface{}{
			"orchestration_type": eventType,
		},
	}
	
	if details != nil {
		for k, v := range details {
			event.Details[k] = v
		}
	}
	
	sl.LogEvent(event)
}