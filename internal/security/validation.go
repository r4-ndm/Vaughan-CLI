package security

import (
	"fmt"
	"net/url"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"unicode"
)

// Validator provides input validation functions
type Validator struct {
	rules map[string]ValidationRule
}

// ValidationRule represents a validation rule
type ValidationRule struct {
	Name        string
	Description string
	Validate    func(interface{}) error
	Required    bool
}

// NewValidator creates a new validator
func NewValidator() *Validator {
	return &Validator{
		rules: make(map[string]ValidationRule),
	}
}

// AddRule adds a validation rule
func (v *Validator) AddRule(name string, rule ValidationRule) {
	rule.Name = name
	v.rules[name] = rule
}

// Validate validates input against all rules
func (v *Validator) Validate(input interface{}) error {
	for name, rule := range v.rules {
		if err := rule.Validate(input); err != nil {
			return fmt.Errorf("validation failed for %s: %w", name, err)
		}
	}
	return nil
}

// ValidateInput validates input with context
func (v *Validator) ValidateInput(input interface{}, context string) error {
	for name, rule := range v.rules {
		if err := rule.Validate(input); err != nil {
			return fmt.Errorf("%s: validation failed for %s: %w", context, name, err)
		}
	}
	return nil
}

// Common validation rules
var (
	// NotEmpty validates that input is not empty
	NotEmpty = ValidationRule{
		Name:        "not_empty",
		Description: "Input cannot be empty",
		Validate: func(input interface{}) error {
			if input == nil {
				return fmt.Errorf("input is required")
			}
			switch v := input.(type) {
			case string:
				if strings.TrimSpace(v) == "" {
					return fmt.Errorf("input cannot be empty string")
				}
			case []interface{}:
				if len(v) == 0 {
					return fmt.Errorf("input cannot be empty slice")
				}
			}
			return nil
		},
		Required: true,
	}

	// URL validates URL format
	URL = ValidationRule{
		Name:        "url",
		Description: "Input must be a valid URL",
		Validate: func(input interface{}) error {
			str, ok := input.(string)
			if !ok {
				return fmt.Errorf("input must be string")
			}
			
			if _, err := url.ParseRequestURI(str); err != nil {
				return fmt.Errorf("invalid URL format: %w", err)
			}
			
			// Only allow http/https
			if !strings.HasPrefix(str, "http://") && !strings.HasPrefix(str, "https://") {
				return fmt.Errorf("URL must use http or https")
			}
			
			return nil
		},
		Required: false,
	}

	// EthereumAddress validates Ethereum address format
	EthereumAddress = ValidationRule{
		Name:        "ethereum_address",
		Description: "Input must be a valid Ethereum address",
		Validate: func(input interface{}) error {
			str, ok := input.(string)
			if !ok {
				return fmt.Errorf("input must be string")
			}
			
			// Basic validation (0x prefix + 40 hex chars)
			ethAddrRegex := regexp.MustCompile(`^0x[a-fA-F0-9]{40}$`)
			if !ethAddrRegex.MatchString(str) {
				return fmt.Errorf("invalid Ethereum address format")
			}
			
			return nil
		},
		Required: false,
	}

	// SafePath validates file path to prevent traversal
	SafePath = ValidationRule{
		Name:        "safe_path",
		Description: "Input path must be safe (no traversal)",
		Validate: func(input interface{}) error {
			str, ok := input.(string)
			if !ok {
				return fmt.Errorf("input must be string")
			}
			
			// Check for path traversal
			if strings.Contains(str, "..") {
				return fmt.Errorf("path traversal detected")
			}
			
			// Check for absolute paths (restrict to working directory)
			if filepath.IsAbs(str) {
				return fmt.Errorf("absolute paths not allowed")
			}
			
			// Check for dangerous characters
			dangerousChars := []string{"<", ">", "|", "&", ";", "`", "$", "(", ")", "{", "}", "[", "]"}
			for _, char := range dangerousChars {
				if strings.Contains(str, char) {
					return fmt.Errorf("dangerous character '%s' in path", char)
				}
			}
			
			return nil
		},
		Required: false,
	}

	// NonNegativeInt validates non-negative integers
	NonNegativeInt = ValidationRule{
		Name:        "non_negative_int",
		Description: "Input must be non-negative integer",
		Validate: func(input interface{}) error {
			var num int
			switch v := input.(type) {
			case int:
				num = v
			case float64:
				num = int(v)
			case string:
				parsed, err := strconv.Atoi(v)
				if err != nil {
					return fmt.Errorf("invalid integer format")
				}
				num = parsed
			default:
				return fmt.Errorf("input must be number")
			}
			
			if num < 0 {
				return fmt.Errorf("input must be non-negative")
			}
			
			return nil
		},
		Required: false,
	}

	// SafeText validates text for injection prevention
	SafeText = ValidationRule{
		Name:        "safe_text",
		Description: "Input must not contain dangerous characters",
		Validate: func(input interface{}) error {
			str, ok := input.(string)
			if !ok {
				return fmt.Errorf("input must be string")
			}
			
			// Check for dangerous patterns
			dangerousPatterns := []string{
				`<script.*?>.*?</script>`, // XSS
				`javascript:`,             // JS injection
				`on\w+\s*=`,               // Event handlers
			}
			
			for _, pattern := range dangerousPatterns {
				if matched, _ := regexp.MatchString(pattern, str); matched {
					return fmt.Errorf("dangerous pattern detected")
				}
			}
			
			return nil
		},
		Required: false,
	}

	// RPCURL validates RPC endpoint URLs
	RPCURL = ValidationRule{
		Name:        "rpc_url",
		Description: "Input must be a valid RPC URL",
		Validate: func(input interface{}) error {
			str, ok := input.(string)
			if !ok {
				return fmt.Errorf("input must be string")
			}
			
			// Use URL validation first
			if err := URL.Validate(input); err != nil {
				return err
			}
			
			// Additional RPC-specific validation
			if !strings.Contains(str, "rpc") {
				return fmt.Errorf("URL should contain 'rpc' for RPC endpoints")
			}
			
			// Prefer HTTPS
			if strings.HasPrefix(str, "http://") {
				return fmt.Errorf("HTTPS recommended for RPC endpoints")
			}
			
			return nil
		},
		Required: false,
	}
)

// ValidateTransactionParams validates transaction parameters
func ValidateTransactionParams(to, value, data interface{}) error {
	// Validate recipient address
	toValidator := NewValidator()
	toValidator.AddRule("to_address", NotEmpty)
	toValidator.AddRule("to_address", EthereumAddress)
	
	if err := toValidator.ValidateInput(to, "recipient address"); err != nil {
		return err
	}
	
	// Validate value
	if value != nil {
		valueValidator := NewValidator()
		valueValidator.AddRule("value", NonNegativeInt)
		
		if err := valueValidator.ValidateInput(value, "transaction value"); err != nil {
			return err
		}
	}
	
	return nil
}

// ValidateConfigPath validates configuration file path
func ValidateConfigPath(path interface{}) error {
	validator := NewValidator()
	validator.AddRule("config_path", NotEmpty)
	validator.AddRule("config_path", SafePath)
	
	return validator.ValidateInput(path, "configuration path")
}

// SanitizeInput sanitizes user input
func SanitizeInput(input string) string {
	// Remove null bytes
	input = strings.ReplaceAll(input, "\x00", "")
	
	// Remove control characters except newline and tab
	result := strings.Builder{}
	for _, r := range input {
		if unicode.IsGraphic(r) || r == '\n' || r == '\t' {
			result.WriteRune(r)
		}
	}
	
	return strings.TrimSpace(result.String())
}

// ValidateNetworkParams validates blockchain network parameters
func ValidateNetworkParams(name, rpcURL, chainID interface{}) error {
	validator := NewValidator()
	validator.AddRule("network_name", NotEmpty)
	validator.AddRule("rpc_url", RPCURL)
	validator.AddRule("chain_id", NonNegativeInt)
	
	if err := validator.ValidateInput(name, "network name"); err != nil {
		return err
	}
	
	if err := validator.ValidateInput(rpcURL, "RPC URL"); err != nil {
		return err
	}
	
	if chainID != nil {
		if err := validator.ValidateInput(chainID, "chain ID"); err != nil {
			return err
		}
	}
	
	return nil
}
