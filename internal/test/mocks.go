package test

import (
	"context"
	"fmt"
	"time"

	"github.com/r4v3n/vaughan-cli/internal/fantasy"
	"github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// MockBlockchainNetwork implements BlockchainNetwork interface for testing
type MockBlockchainNetwork struct {
	NameValue        string
	ChainIDValue     int
	RPCUrlValue      string
	GasTokenValue    string
	BlockTimeValue   int
	ExplorerValue    string
	TypeValue        string
}

func (m *MockBlockchainNetwork) GetName() string {
	return m.NameValue
}

func (m *MockBlockchainNetwork) GetChainID() int {
	return m.ChainIDValue
}

func (m *MockBlockchainNetwork) GetRPCURL() string {
	return m.RPCUrlValue
}

func (m *MockBlockchainNetwork) GetGasToken() string {
	return m.GasTokenValue
}

func (m *MockBlockchainNetwork) GetBlockTime() int {
	return m.BlockTimeValue
}

func (m *MockBlockchainNetwork) GetExplorerURL() string {
	return m.ExplorerValue
}

func (m *MockBlockchainNetwork) GetType() string {
	return m.TypeValue
}

// NewMockBlockchainNetwork creates a mock network with default values
func NewMockBlockchainNetwork() *MockBlockchainNetwork {
	return &MockBlockchainNetwork{
		NameValue:        "Test Network",
		ChainIDValue:     12345,
		RPCUrlValue:      "https://test.rpc.example.com",
		GasTokenValue:    "TEST",
		BlockTimeValue:   5,
		ExplorerValue:    "https://test.explorer.example.com",
		TypeValue:        "testnet",
	}
}

// MockConfigManager implements ConfigManager interface for testing
type MockConfigManager struct {
	NetworksValue  map[string]interfaces.BlockchainNetwork
	DefaultNetwork string
	ErrorValue     error
	CallCount     int
	LoadCalls     int
	SaveCalls     int
}

func (m *MockConfigManager) Load() error {
	m.CallCount++
	m.LoadCalls++
	return m.ErrorValue
}

func (m *MockConfigManager) Save() error {
	m.CallCount++
	m.SaveCalls++
	return m.ErrorValue
}

func (m *MockConfigManager) GetNetwork(networkName string) (interfaces.BlockchainNetwork, error) {
	m.CallCount++
	if m.ErrorValue != nil {
		return nil, m.ErrorValue
	}

	network, exists := m.NetworksValue[networkName]
	if !exists {
		return nil, fmt.Errorf("network not found: %s", networkName)
	}

	return network, nil
}

func (m *MockConfigManager) GetNetworks() []interfaces.BlockchainNetwork {
	m.CallCount++
	var networks []interfaces.BlockchainNetwork
	for _, network := range m.NetworksValue {
		networks = append(networks, network)
	}
	return networks
}

func (m *MockConfigManager) GetDefaultNetwork() string {
	m.CallCount++
	return m.DefaultNetwork
}

func (m *MockConfigManager) SetDefaultNetwork(networkName string) error {
	m.CallCount++
	if m.ErrorValue != nil {
		return m.ErrorValue
	}

	m.DefaultNetwork = networkName
	return nil
}

func (m *MockConfigManager) Validate() error {
	m.CallCount++
	return m.ErrorValue
}

// NewMockConfigManager creates a mock config manager with default values
func NewMockConfigManager() *MockConfigManager {
	return &MockConfigManager{
		NetworksValue: make(map[string]interfaces.BlockchainNetwork),
		DefaultNetwork: "testnet",
		ErrorValue:     nil,
	}
}

// MockToolProvider implements ToolProvider interface for testing
type MockToolProvider struct {
	ToolsValue     []fantasy.AgentTool
	RegisteredTools map[string]fantasy.AgentTool
	ErrorValue     error
	CallCount      int
}

func (m *MockToolProvider) GetName() string {
	m.CallCount++
	return "Mock Tool Provider"
}

func (m *MockToolProvider) GetDescription() string {
	m.CallCount++
	return "Mock tool provider for testing"
}

func (m *MockToolProvider) GetTools() []fantasy.AgentTool {
	m.CallCount++
	return m.ToolsValue
}

func (m *MockToolProvider) RegisterTool(tool fantasy.AgentTool) error {
	m.CallCount++
	if m.ErrorValue != nil {
		return m.ErrorValue
	}

	if m.RegisteredTools == nil {
		m.RegisteredTools = make(map[string]fantasy.AgentTool)
	}

	m.RegisteredTools[tool.GetName()] = tool
	return nil
}

func (m *MockToolProvider) UnregisterTool(toolName string) error {
	m.CallCount++
	if m.ErrorValue != nil {
		return m.ErrorValue
	}

	delete(m.RegisteredTools, toolName)
	return nil
}

// NewMockToolProvider creates a mock tool provider
func NewMockToolProvider() *MockToolProvider {
	return &MockToolProvider{
		ToolsValue:      make([]fantasy.AgentTool, 0),
		RegisteredTools:  make(map[string]fantasy.AgentTool),
		ErrorValue:      nil,
	}
}

// MockModelProvider implements ModelProvider interface for testing
type MockModelProvider struct {
	NameValue     string
	TypeValue     string
	ModelValue    string
	EndpointValue string
	ErrorValue    error
	CallCount     int
}

func (m *MockModelProvider) GetName() string {
	m.CallCount++
	return m.NameValue
}

func (m *MockModelProvider) GetType() string {
	m.CallCount++
	return m.TypeValue
}

func (m *MockModelProvider) GetModel() string {
	m.CallCount++
	return m.ModelValue
}

func (m *MockModelProvider) GetEndpoint() string {
	m.CallCount++
	return m.EndpointValue
}

func (m *MockModelProvider) Generate(ctx context.Context, prompt string) (string, error) {
	m.CallCount++
	return "Mock response", m.ErrorValue
}

func (m *MockModelProvider) IsAvailable() bool {
	m.CallCount++
	return m.ErrorValue == nil
}

// NewMockModelProvider creates a mock model provider
func NewMockModelProvider() *MockModelProvider {
	return &MockModelProvider{
		NameValue:     "Mock Model",
		TypeValue:     "mock",
		ModelValue:    "mock-model",
		EndpointValue: "http://mock.endpoint",
		ErrorValue:    nil,
	}
}

// MockCastExecutor implements CastExecutor interface for testing
type MockCastExecutor struct {
	CommandOutput map[string]string
	ErrorValue    error
	CallCount     int
}

func (m *MockCastExecutor) ExecuteCommand(ctx context.Context, command string, args []string, options map[string]string) (string, error) {
	m.CallCount++
	if m.ErrorValue != nil {
		return "", m.ErrorValue
	}

	if output, exists := m.CommandOutput[command]; exists {
		return output, nil
	}

	return fmt.Sprintf("Mock output for command: %s", command), nil
}

func (m *MockCastExecutor) ValidateCommand(command string, args []string) error {
	m.CallCount++
	return m.ErrorValue
}

func (m *MockCastExecutor) GetSupportedCommands() []string {
	m.CallCount++
	return []string{"balance", "send", "gas-price", "call", "block"}
}

// NewMockCastExecutor creates a mock cast executor
func NewMockCastExecutor() *MockCastExecutor {
	return &MockCastExecutor{
		CommandOutput: make(map[string]string),
		ErrorValue:    nil,
	}
}

// MockErrorHandler implements ErrorHandler interface for testing
type MockErrorHandler struct {
	ErrorsLogged []ErrorLogEntry
	UserMessages map[string]string
	RetryResults map[error]bool
	CallCount    int
}

type ErrorLogEntry struct {
	Error   error
	Context string
	Time    time.Time
}

func (m *MockErrorHandler) HandleError(err error, context string) error {
	m.CallCount++
	m.ErrorsLogged = append(m.ErrorsLogged, ErrorLogEntry{
		Error:   err,
		Context: context,
		Time:    time.Now(),
	})
	return err
}

func (m *MockErrorHandler) LogError(err error, context string) {
	m.CallCount++
	m.HandleError(err, context)
}

func (m *MockErrorHandler) GetUserMessage(err error) string {
	m.CallCount++
	if msg, exists := m.UserMessages[err.Error()]; exists {
		return msg
	}
	return err.Error()
}

func (m *MockErrorHandler) ShouldRetry(err error) bool {
	m.CallCount++
	if retry, exists := m.RetryResults[err]; exists {
		return retry
	}
	return false
}

// NewMockErrorHandler creates a mock error handler
func NewMockErrorHandler() *MockErrorHandler {
	return &MockErrorHandler{
		ErrorsLogged: make([]ErrorLogEntry, 0),
		UserMessages: make(map[string]string),
		RetryResults: make(map[error]bool),
	}
}

// MockSessionManager implements SessionManager interface for testing
type MockSessionManager struct {
	Sessions      map[string]Session
	SessionCounts map[string]int
	ErrorValue    error
	CallCount     int
}

func (m *MockSessionManager) CreateSession(userID string) (string, error) {
	m.CallCount++
	if m.ErrorValue != nil {
		return "", m.ErrorValue
	}

	sessionID := fmt.Sprintf("session-%d-%s", m.CallCount, userID)
	session := Session{
		ID:        sessionID,
		UserID:    userID,
		CreatedAt: time.Now(),
		UpdatedAt: time.Now(),
		Data:      make(map[string]interface{}),
	}

	m.Sessions[sessionID] = session
	m.SessionCounts[userID]++

	return sessionID, nil
}

func (m *MockSessionManager) GetSession(sessionID string) (Session, error) {
	m.CallCount++
	if m.ErrorValue != nil {
		return Session{}, m.ErrorValue
	}

	session, exists := m.Sessions[sessionID]
	if !exists {
		return Session{}, fmt.Errorf("session not found: %s", sessionID)
	}

	return session, nil
}

func (m *MockSessionManager) UpdateSession(sessionID string, updates SessionUpdate) error {
	m.CallCount++
	if m.ErrorValue != nil {
		return m.ErrorValue
	}

	session, exists := m.Sessions[sessionID]
	if !exists {
		return fmt.Errorf("session not found: %s", sessionID)
	}

	if updates.Network != nil {
		session.Data["network"] = *updates.Network
	}
	if updates.Model != nil {
		session.Data["model"] = *updates.Model
	}
	if updates.Preferences != nil {
		session.Data["preferences"] = *updates.Preferences
	}

	session.UpdatedAt = time.Now()
	m.Sessions[sessionID] = session

	return nil
}

func (m *MockSessionManager) DeleteSession(sessionID string) error {
	m.CallCount++
	if m.ErrorValue != nil {
		return m.ErrorValue
	}

	if _, exists := m.Sessions[sessionID]; !exists {
		return fmt.Errorf("session not found: %s", sessionID)
	}

	delete(m.Sessions, sessionID)
	return nil
}

func (m *MockSessionManager) ListSessions(userID string) ([]Session, error) {
	m.CallCount++
	if m.ErrorValue != nil {
		return nil, m.ErrorValue
	}

	var userSessions []Session
	for _, session := range m.Sessions {
		if session.UserID == userID {
			userSessions = append(userSessions, session)
		}
	}

	return userSessions, nil
}

// NewMockSessionManager creates a mock session manager
func NewMockSessionManager() *MockSessionManager {
	return &MockSessionManager{
		Sessions:      make(map[string]Session),
		SessionCounts: make(map[string]int),
		ErrorValue:    nil,
	}
}