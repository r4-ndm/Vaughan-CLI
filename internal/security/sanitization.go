package security

import (
	"encoding/json"
	"fmt"
	"html"
	"net/url"
	"regexp"
	"strings"
	"unicode"
)

// Sanitizer sanitizes and validates outputs
type Sanitizer struct {
	rules []SanitizationRule
}

// SanitizationRule represents a sanitization rule
type SanitizationRule struct {
	Name        string
	Description string
	Sanitize    func(string) string
}

// NewSanitizer creates a new output sanitizer
func NewSanitizer() *Sanitizer {
	return &Sanitizer{
		rules: []SanitizationRule{},
	}
}

// AddRule adds a sanitization rule
func (s *Sanitizer) AddRule(name string, rule SanitizationRule) {
	rule.Name = name
	s.rules = append(s.rules, rule)
}

// Sanitize applies all sanitization rules
func (s *Sanitizer) Sanitize(input string) string {
	result := input
	
	for _, rule := range s.rules {
		result = rule.Sanitize(result)
	}
	
	return result
}

// SanitizeForHTML sanitizes content for HTML output
func (s *Sanitizer) SanitizeForHTML(input string) string {
	// Escape HTML entities
	result := html.EscapeString(input)
	
	// Remove dangerous HTML tags
	dangerousTags := []string{
		`<script[^>]*>.*?</script>`,
		`<iframe[^>]*>.*?</iframe>`,
		`<object[^>]*>.*?</object>`,
		`<embed[^>]*>.*?</embed>`,
		`<form[^>]*>.*?</form>`,
	}
	
	for _, pattern := range dangerousTags {
		re := regexp.MustCompile(`(?is)` + pattern)
		result = re.ReplaceAllString(result, "")
	}
	
	return result
}

// SanitizeForJSON sanitizes content for JSON output
func (s *Sanitizer) SanitizeForJSON(input string) string {
	// Remove null bytes
	result := strings.ReplaceAll(input, "\x00", "")
	
	// Remove control characters
	result = strings.Map(func(r rune) rune {
		if unicode.IsControl(r) && r != '\t' && r != '\n' && r != '\r' {
			return -1
		}
		return r
	}, result)
	
	// Validate JSON
	var js interface{}
	if err := json.Unmarshal([]byte(result), &js); err != nil {
		// If invalid JSON, return safe placeholder
		return `"Invalid JSON content"`
	}
	
	// Re-marshal to ensure valid JSON
	validated, _ := json.Marshal(js)
	return string(validated)
}

// SanitizeForCLI sanitizes content for CLI output
func (s *Sanitizer) SanitizeForCLI(input string) string {
	result := input
	
	// Remove ANSI escape codes (potential injection)
	ansiEscape := regexp.MustCompile(`\x1b\[[0-9;]*[mGKHJABCD]`)
	result = ansiEscape.ReplaceAllString(result, "")
	
	// Remove control characters that could affect terminal
	controlChars := regexp.MustCompile(`[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]`)
	result = controlChars.ReplaceAllString(result, "")
	
	return result
}

// SanitizeURL sanitizes URL output
func (s *Sanitizer) SanitizeURL(input string) string {
	// Parse and validate URL
	parsed, err := url.Parse(input)
	if err != nil {
		return "[Invalid URL]"
	}
	
	// Only allow safe schemes
	safeSchemes := map[string]bool{
		"http":  true,
		"https": true,
	}
	
	if !safeSchemes[parsed.Scheme] {
		return "[Unsafe URL scheme]"
	}
	
	// Remove user info (passwords)
	parsed.User = nil
	
	return parsed.String()
}

// SanitizeAPIKey sanitizes API key output
func (s *Sanitizer) SanitizeAPIKey(input string) string {
	if len(input) < 8 {
		return "***"
	}
	
	// Show first 4 and last 4 characters
	return input[:4] + strings.Repeat("*", len(input)-8) + input[len(input)-4:]
}

// SanitizePrivateKey sanitizes private key output
func (s *Sanitizer) SanitizePrivateKey(input string) string {
	return strings.Repeat("*", len(input))
}

// Common sanitization rules
var (
	// RemoveNullBytes removes null bytes from input
	RemoveNullBytes = SanitizationRule{
		Name:        "remove_null_bytes",
		Description: "Remove null bytes from input",
		Sanitize: func(input string) string {
			return strings.ReplaceAll(input, "\x00", "")
		},
	}
	
	// RemoveControlChars removes control characters except newline and tab
	RemoveControlChars = SanitizationRule{
		Name:        "remove_control_chars",
		Description: "Remove control characters except newline and tab",
		Sanitize: func(input string) string {
			return strings.Map(func(r rune) rune {
				if unicode.IsControl(r) && r != '\n' && r != '\t' {
					return -1
				}
				return r
			}, input)
		},
	}
	
	// TrimWhitespace trims excessive whitespace
	TrimWhitespace = SanitizationRule{
		Name:        "trim_whitespace",
		Description: "Trim excessive whitespace",
		Sanitize: func(input string) string {
			// Replace multiple spaces with single space
			result := regexp.MustCompile(`\s+`).ReplaceAllString(input, " ")
			// Trim leading/trailing whitespace
			return strings.TrimSpace(result)
		},
	}
	
	// RemoveExcessNewlines removes excessive newlines
	RemoveExcessNewlines = SanitizationRule{
		Name:        "remove_excess_newlines",
		Description: "Remove excessive newlines",
		Sanitize: func(input string) string {
			// Replace 3+ newlines with 2 newlines
			result := regexp.MustCompile(`\n{3,}`).ReplaceAllString(input, "\n\n")
			// Trim leading/trailing newlines
			return strings.TrimSpace(result)
		},
	}
)

// SanitizeToolOutput sanitizes tool output based on type
func SanitizeToolOutput(toolName string, output interface{}) (string, error) {
	sanitizer := NewSanitizer()
	
	// Add common rules
	sanitizer.AddRule("null_bytes", RemoveNullBytes)
	sanitizer.AddRule("control_chars", RemoveControlChars)
	sanitizer.AddRule("whitespace", TrimWhitespace)
	
	// Convert output to string
	var outputStr string
	switch v := output.(type) {
	case string:
		outputStr = v
	case []byte:
		outputStr = string(v)
	case fmt.Stringer:
		outputStr = v.String()
	default:
		jsonBytes, err := json.Marshal(v)
		if err != nil {
			return "", fmt.Errorf("failed to serialize output")
		}
		outputStr = string(jsonBytes)
	}
	
	// Apply basic sanitization
	result := sanitizer.Sanitize(outputStr)
	
	// Apply tool-specific sanitization
	switch toolName {
	case "cast_balance":
		// Sanitize balance output (JSON)
		result = sanitizer.SanitizeForJSON(result)
	case "download", "fetch":
		// Sanitize file content
		result = sanitizer.SanitizeForCLI(result)
	case "view", "ls":
		// Sanitize file system output
		result = sanitizer.SanitizeForCLI(result)
	default:
		// Default CLI sanitization
		result = sanitizer.SanitizeForCLI(result)
	}
	
	return result, nil
}

// SanitizeUserPrompt sanitizes user prompts before AI processing
func SanitizeUserPrompt(prompt string) string {
	sanitizer := NewSanitizer()
	
	// Remove dangerous patterns
	dangerousPatterns := []SanitizationRule{
		{
			Name: "remove_javascript",
			Sanitize: func(input string) string {
				jsPattern := regexp.MustCompile(`(?i)javascript:`)
				return jsPattern.ReplaceAllString(input, "[removed]")
			},
		},
		{
			Name: "remove_sql_injection",
			Sanitize: func(input string) string {
				sqlPatterns := []string{
					`(?i)(union\s+select)`,
					`(?i)(drop\s+table)`,
					`(?i)(delete\s+from)`,
					`(?i)(insert\s+into)`,
				}
				result := input
				for _, pattern := range sqlPatterns {
					re := regexp.MustCompile(pattern)
					result = re.ReplaceAllString(result, "[removed]")
				}
				return result
			},
		},
	}
	
	for _, rule := range dangerousPatterns {
		sanitizer.AddRule(rule.Name, rule)
	}
	
	return sanitizer.Sanitize(prompt)
}

// DefaultSanitizer creates a sanitizer with common rules
func DefaultSanitizer() *Sanitizer {
	sanitizer := NewSanitizer()
	sanitizer.AddRule("null_bytes", RemoveNullBytes)
	sanitizer.AddRule("control_chars", RemoveControlChars)
	sanitizer.AddRule("whitespace", TrimWhitespace)
	sanitizer.AddRule("newlines", RemoveExcessNewlines)
	
	return sanitizer
}
