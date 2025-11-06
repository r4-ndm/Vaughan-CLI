package test

import (
	"os"
	"testing"

	"github.com/stretchr/testify/assert"
)

// AssertNoError checks if error is nil
func AssertNoError(t *testing.T, err error) {
	assert.NoError(t, err)
}

// AssertError checks if error is not nil
func AssertError(t *testing.T, err error) {
	assert.Error(t, err)
}

// AssertEqual checks if two values are equal
func AssertEqual(t *testing.T, expected, actual interface{}) {
	assert.Equal(t, expected, actual)
}

// AssertNotEqual checks if two values are not equal
func AssertNotEqual(t *testing.T, expected, actual interface{}) {
	assert.NotEqual(t, expected, actual)
}

// AssertTrue checks if condition is true
func AssertTrue(t *testing.T, condition bool) {
	assert.True(t, condition)
}

// AssertFalse checks if condition is false
func AssertFalse(t *testing.T, condition bool) {
	assert.False(t, condition)
}

// AssertContains checks if slice/map contains expected value
func AssertContains(t *testing.T, collection interface{}, expected interface{}) {
	assert.Contains(t, collection, expected)
}

// AssertNil checks if value is nil
func AssertNil(t *testing.T, value interface{}) {
	assert.Nil(t, value)
}

// AssertNotNil checks if value is not nil
func AssertNotNil(t *testing.T, value interface{}) {
	assert.NotNil(t, value)
}

// AssertPanics checks if function panics
func AssertPanics(t *testing.T, f func()) {
	assert.Panics(t, f)
}

// AssertEmpty checks if value is empty
func AssertEmpty(t *testing.T, value interface{}) {
	assert.Empty(t, value)
}

// TempFile creates a temporary file for testing
func TempFile(t *testing.T, content string) string {
	tmpFile, err := os.CreateTemp("", "test-*.json")
	if err != nil {
		t.Fatalf("Failed to create temp file: %v", err)
	}

	if _, err := tmpFile.WriteString(content); err != nil {
		t.Fatalf("Failed to write temp file: %v", err)
	}

	return tmpFile.Name()
}

// TempDir creates a temporary directory for testing
func TempDir(t *testing.T) string {
	tmpDir, err := os.MkdirTemp("", "test-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}

	return tmpDir
}

// CleanupTemp removes temporary files and directories
func CleanupTemp(t *testing.T, paths ...string) {
	for _, path := range paths {
		if err := os.RemoveAll(path); err != nil {
			t.Logf("Failed to cleanup temp path %s: %v", path, err)
		}
	}
}

// SkipIfShort skips test if running with -short flag
func SkipIfShort(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping test in short mode")
	}
}

// SkipTest skips a test with a reason
func SkipTest(t *testing.T, reason string) {
	t.Skip(reason)
}

// TestFunction represents a test function
type TestFunction func(t *testing.T)

// TableTest represents a table-driven test
type TableTest struct {
	Name     string
	Input    interface{}
	Expected interface{}
	TestFunc TestFunction
}

// RunTableTests runs multiple test cases
func RunTableTests(t *testing.T, tests []TableTest) {
	for _, test := range tests {
		t.Run(test.Name, func(t *testing.T) {
			if test.TestFunc != nil {
				test.TestFunc(t)
			}
		})
	}
}

// MockInterface creates a mock interface using reflection
func MockInterface(t *testing.T, targetType interface{}) interface{} {
	// This is a placeholder for advanced mocking
	// In real implementation, you might use testify/mock
	// or generate mocks using tools like mockery
	t.Skip("Advanced mocking not implemented in this helper")
	return nil
}

// SetupTestEnvironment sets up test environment variables
func SetupTestEnvironment(t *testing.T, envVars map[string]string) {
	for key, value := range envVars {
		os.Setenv(key, value)
	}

	// Return cleanup function
	t.Cleanup(func() {
		for key := range envVars {
			os.Unsetenv(key)
		}
	})
}

// CleanupFiles removes multiple files
func CleanupFiles(t *testing.T, files ...string) {
	for _, file := range files {
		if err := os.Remove(file); err != nil && !os.IsNotExist(err) {
			t.Logf("Failed to remove file %s: %v", file, err)
		}
	}
}

// AssertFileExists checks if file exists
func AssertFileExists(t *testing.T, filename string) {
	_, err := os.Stat(filename)
	assert.NoError(t, err, "File should exist: %s", filename)
}

// AssertFileNotExists checks if file doesn't exist
func AssertFileNotExists(t *testing.T, filename string) {
	_, err := os.Stat(filename)
	assert.Error(t, err, "File should not exist: %s", filename)
	assert.True(t, os.IsNotExist(err), "Error should be file not found: %s", filename)
}

// ReadFile reads file contents for testing
func ReadFile(t *testing.T, filename string) string {
	content, err := os.ReadFile(filename)
	assert.NoError(t, err, "Failed to read file: %s", filename)
	return string(content)
}

// WriteFile writes file contents for testing
func WriteFile(t *testing.T, filename, content string) {
	err := os.WriteFile(filename, []byte(content), 0644)
	assert.NoError(t, err, "Failed to write file: %s", filename)
}

// CreateTestConfig creates a test configuration
func CreateTestConfig(t *testing.T) string {
	config := `{
		"models": {
			"large": {
				"model": "test-model",
				"provider": "test-provider"
			}
		},
		"blockchain": {
			"default_network": "testnet",
			"networks": {
				"testnet": {
					"name": "Test Network",
					"chain_id": 12345,
					"rpc_url": "https://test.rpc",
					"block_time": 5,
					"gas_token": "TEST",
					"type": "testnet"
				}
			}
		}
	}`
	return TempFile(t, config)
}

// CaptureOutput captures function output for testing
func CaptureOutput(t *testing.T, f func()) string {
	// This would typically use pipe or capture.Stdout
	// For now, it's a placeholder
	t.Skip("Output capture not implemented in this helper")
	return ""
}

// MockService creates a mock service for testing
func MockService(t *testing.T, serviceName string, implementation interface{}) {
	t.Logf("Creating mock service: %s", serviceName)
	// This would typically register with a dependency container
}

// BenchmarkHelper measures function execution time
func BenchmarkHelper(t *testing.T, name string, iterations int, f func()) {
	t.Helper()
	t.Run(name, func(b *testing.B) {
		b.ResetTimer()
		for i := 0; i < iterations; i++ {
			f()
		}
		b.StopTimer()
	})
}

// AssertWithinRange checks if value is within expected range
func AssertWithinRange(t *testing.T, value, min, max float64) {
	assert.GreaterOrEqual(t, value, min, "Value should be >= min")
	assert.LessOrEqual(t, value, max, "Value should be <= max")
}

// AssertType checks if value is of expected type
func AssertType(t *testing.T, value, expectedType interface{}) {
	assert.IsType(t, expectedType, value)
}

// WaitForCondition waits for condition to be true (with timeout)
func WaitForCondition(t *testing.T, condition func() bool, timeout time.Duration) bool {
	start := time.Now()
	for time.Since(start) < timeout {
		if condition() {
			return true
		}
		time.Sleep(10 * time.Millisecond)
	}
	return false
}

// AssertEventuallyTrue checks if condition becomes true within timeout
func AssertEventuallyTrue(t *testing.T, condition func() bool, timeout time.Duration, message string) {
	assert.True(t, WaitForCondition(t, condition, timeout), message)
}