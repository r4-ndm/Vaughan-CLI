package errors

import (
	"fmt"
)

// Error types for Vaughan Crush
var (
	// Configuration errors
	ErrConfigNotFound       = New("configuration file not found")
	ErrConfigInvalid       = New("invalid configuration")
	ErrNetworkNotFound     = New("network not found")
	ErrModelNotFound       = New("model not found")
	ErrProviderNotFound    = New("provider not found")

	// Blockchain errors
	ErrTransactionFailed  = New("transaction failed")
	ErrInsufficientGas    = New("insufficient gas")
	ErrAddressInvalid     = New("invalid address")
	ErrContractNotFound    = New("contract not found")
	ErrNetworkMismatch    = New("network mismatch")

	// Tool errors
	ErrToolNotFound       = New("tool not found")
	ErrToolExecution     = New("tool execution failed")
	ErrInvalidInput       = New("invalid input")
	ErrPermissionDenied   = New("permission denied")

	// Model errors
	ErrModelUnavailable  = New("model unavailable")
	ErrGenerationFailed  = New("generation failed")
	ErrContextTooLarge   = New("context too large")

	// System errors
	ErrFileNotFound       = New("file not found")
	ErrFilePermission     = New("file permission denied")
	ErrNetworkUnavailable = New("network unavailable")
)

// Error represents a Vaughan Crush error
type Error struct {
	Code    string
	Message string
	Cause   error
	Context map[string]interface{}
}

// New creates a new Error
func New(message string) *Error {
	return &Error{
		Code:    generateErrorCode(message),
		Message: message,
		Context: make(map[string]interface{}),
	}
}

// WithCause adds a cause to the error
func (e *Error) WithCause(cause error) *Error {
	e.Cause = cause
	return e
}

// WithContext adds context to the error
func (e *Error) WithContext(key string, value interface{}) *Error {
	e.Context[key] = value
	return e
}

// Error implements error interface
func (e *Error) Error() string {
	if e.Cause != nil {
		return fmt.Sprintf("%s: %s (caused by: %s)", e.Code, e.Message, e.Cause.Error())
	}
	return fmt.Sprintf("%s: %s", e.Code, e.Message)
}

// Is checks if error matches target
func (e *Error) Is(target error) bool {
	if t, ok := target.(*Error); ok {
		return e.Code == t.Code
	}
	return false
}

// Unwrap returns the cause of the error
func (e *Error) Unwrap() error {
	return e.Cause
}

// GetContext returns the error context
func (e *Error) GetContext() map[string]interface{} {
	return e.Context
}

// GetUserMessage returns a user-friendly error message
func (e *Error) GetUserMessage() string {
	if msg, exists := userMessages[e.Code]; exists {
		if len(e.Context) > 0 {
			return fmt.Sprintf(msg, e.Context)
		}
		return msg
	}
	return e.Message
}

// ShouldRetry determines if the operation should be retried
func (e *Error) ShouldRetry() bool {
	_, exists := retryableErrors[e.Code]
	return exists
}

// generateErrorCode generates error code from message
func generateErrorCode(message string) string {
	// Simple hash-like generation for error codes
	code := "VC"
	for _, char := range message {
		code += fmt.Sprintf("%02X", char%16)
	}
	return code[:6] // Keep codes short
}

// User-friendly error messages
var userMessages = map[string]string{
	"VCNF":  "Configuration file not found. Please check the file path and permissions.",
	"VCNI":  "Network '%s' is not available. Please check the network configuration.",
	"VCMF":  "Model '%s' is not available. Please check the model configuration.",
	"VCIT":  "Insufficient gas for this transaction. Please increase the gas limit or gas price.",
	"VCAF":  "Address '%s' is invalid. Please check the address format.",
	"VCTF":  "Transaction failed: %s. Please check the transaction details and try again.",
	"VCEU":  "Unable to connect to the network. Please check your internet connection and RPC URL.",
}

// Retryable errors
var retryableErrors = map[string]bool{
	"VCEU": true, // Network unavailable
	"VCTF": true, // Transaction failed (might be temporary)
}

// Wrap wraps an error in Vaughan Crush error
func Wrap(err error, message string) *Error {
	return New(message).WithCause(err)
}

// WithContext wraps an error with additional context
func WithContext(err error, message string, context map[string]interface{}) *Error {
	vaughanErr := New(message).WithCause(err)
	for key, value := range context {
		vaughanErr.WithContext(key, value)
	}
	return vaughanErr
}

// IsNotFound checks if error is a "not found" type error
func IsNotFound(err error) bool {
	vaughanErr, ok := err.(*Error)
	if !ok {
		return false
	}
	return vaughanErr.Code == ErrConfigNotFound.Code ||
		vaughanErr.Code == ErrNetworkNotFound.Code ||
		vaughanErr.Code == ErrModelNotFound.Code ||
		vaughanErr.Code == ErrToolNotFound.Code
}

// IsRetryable checks if error is retryable
func IsRetryable(err error) bool {
	vaughanErr, ok := err.(*Error)
	if !ok {
		return false
	}
	return vaughanErr.ShouldRetry()
}

// GetUserMessage returns user-friendly message for any error
func GetUserMessage(err error) string {
	vaughanErr, ok := err.(*Error)
	if !ok {
		return err.Error()
	}
	return vaughanErr.GetUserMessage()
}