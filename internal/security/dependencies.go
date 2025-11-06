package security

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"bytes"
	"sync"
	"time"
)

// DependencyScanner scans dependencies for security vulnerabilities
type DependencyScanner struct {
	vulnerabilityDB map[string][]Vulnerability
	mutex           sync.RWMutex
	logger          *SecurityLogger
	cacheDir        string
	scanInterval    time.Duration
}

// Vulnerability represents a security vulnerability
type Vulnerability struct {
	ID           string               `json:"id"`
	Severity     DependencySeverity   `json:"severity"`
	CVE          string               `json:"cve,omitempty"`
	Title        string               `json:"title"`
	Description  string               `json:"description"`
	Published    time.Time            `json:"published"`
	Updated      time.Time            `json:"updated"`
	Affected     string               `json:"affected"`
	Fixed        string               `json:"fixed,omitempty"`
	References   []string             `json:"references"`
	Score        float64              `json:"score"`
	Components   []string             `json:"components"`
	AttackVector string               `json:"attack_vector"`
	Impact       string               `json:"impact"`
}

// DependencySeverity represents vulnerability severity levels
type DependencySeverity string

const (
	DepSeverityCritical DependencySeverity = "critical"
	DepSeverityHigh     DependencySeverity = "high"
	DepSeverityMedium   DependencySeverity = "medium"
	DepSeverityLow      DependencySeverity = "low"
	DepSeverityInfo     DependencySeverity = "info"
)

// Dependency represents a software dependency
type Dependency struct {
	Name         string            `json:"name"`
	Version      string            `json:"version"`
	Hash         string            `json:"hash,omitempty"`
	Source       string            `json:"source"`
	License      string            `json:"license"`
	Dependencies []Dependency      `json:"dependencies,omitempty"`
	Metadata     map[string]string `json:"metadata"`
}

// SBOM (Software Bill of Materials) represents complete dependency information
type SBOM struct {
	Version     string       `json:"version"`
	Generated   time.Time    `json:"generated"`
	Component   string       `json:"component"`
	Hash        string       `json:"hash"`
	Dependencies []Dependency `json:"dependencies"`
	Metadata    map[string]interface{} `json:"metadata"`
}

// ScanResult represents vulnerability scan results
type ScanResult struct {
	SBOM          *SBOM               `json:"sbom"`
	Scanned       time.Time            `json:"scanned"`
	Duration      time.Duration        `json:"duration"`
	Vulnerabilities []Vulnerability    `json:"vulnerabilities"`
	Summary       ScanSummary         `json:"summary"`
	Recommendations []string          `json:"recommendations"`
	NextScan      time.Time           `json:"next_scan"`
}

// ScanSummary provides summary of scan results
type ScanSummary struct {
	TotalDeps        int     `json:"total_deps"`
	VulnerableDeps   int     `json:"vulnerable_deps"`
	CriticalVulns    int     `json:"critical_vulns"`
	HighVulns        int     `json:"high_vulns"`
	MediumVulns      int     `json:"medium_vulns"`
	LowVulns         int     `json:"low_vulns"`
	InfoVulns        int     `json:"info_vulns"`
	LicenseIssues    int     `json:"license_issues"`
	ComplianceScore  float64 `json:"compliance_score"`
	SecurityScore    float64 `json:"security_score"`
}

// DependencyManager manages dependency security
type DependencyManager struct {
	scanner        *DependencyScanner
	sbomPath       string
	logger         *SecurityLogger
	autoUpdate     bool
	scanInterval   time.Duration
}

// NewDependencyScanner creates a new dependency scanner
func NewDependencyScanner(logger *SecurityLogger) *DependencyScanner {
	ds := &DependencyScanner{
		vulnerabilityDB: make(map[string][]Vulnerability),
		logger:          logger,
	}
	
	// Initialize vulnerability database
	ds.initializeVulnDB()
	
	return ds
}

// NewDependencyManager creates dependency security manager
func NewDependencyManager(sbomPath string, logger *SecurityLogger) *DependencyManager {
	return &DependencyManager{
		scanner:      NewDependencyScanner(logger),
		sbomPath:     sbomPath,
		logger:       logger,
		autoUpdate:   true,
		scanInterval: 24 * time.Hour,
	}
}

// ScanDependencies scans dependencies for vulnerabilities
func (ds *DependencyScanner) ScanDependencies(dependencies []Dependency) (*ScanResult, error) {
	start := time.Now()
	
	result := &ScanResult{
		SBOM: &SBOM{
			Version:     "1.0",
			Generated:   start,
			Dependencies: dependencies,
			Metadata:    make(map[string]interface{}),
		},
		Scanned:        start,
		Vulnerabilities: make([]Vulnerability, 0),
		Recommendations: make([]string, 0),
	}
	
	// Scan each dependency
	for _, dep := range dependencies {
		vulns := ds.scanDependency(dep)
		result.Vulnerabilities = append(result.Vulnerabilities, vulns...)
	}
	
	// Generate summary
	result.Summary = ds.generateSummary(dependencies, result.Vulnerabilities)
	
	// Generate recommendations
	result.Recommendations = ds.generateRecommendations(result.Vulnerabilities)
	
	// Set next scan time
	result.NextScan = time.Now().Add(ds.scanInterval)
	result.Duration = time.Since(start)
	
	// Log scan results
	if ds.logger != nil {
		ds.logger.LogDependencyScan(result)
	}
	
	return result, nil
}

// scanDependency scans individual dependency for vulnerabilities
func (ds *DependencyScanner) scanDependency(dep Dependency) []Vulnerability {
	var vulns []Vulnerability
	
	// Check vulnerability database
	if depVulns, exists := ds.vulnerabilityDB[dep.Name]; exists {
		for _, vuln := range depVulns {
			if ds.isVersionAffected(dep.Version, vuln.Affected, vuln.Fixed) {
				vulns = append(vulns, vuln)
			}
		}
	}
	
	return vulns
}

// isVersionAffected checks if version is affected by vulnerability
func (ds *DependencyScanner) isVersionAffected(version, affected, fixed string) bool {
	// Simplified version comparison
	// In production, use proper semantic versioning
	return true // Placeholder
}

// generateSummary generates scan summary
func (ds *DependencyScanner) generateSummary(dependencies []Dependency, vulnerabilities []Vulnerability) ScanSummary {
	summary := ScanSummary{
		TotalDeps:      len(dependencies),
		CriticalVulns:  0,
		HighVulns:      0,
		MediumVulns:    0,
		LowVulns:       0,
		InfoVulns:       0,
	}
	
	vulnDeps := make(map[string]bool)
	for _, vuln := range vulnerabilities {
		vulnDeps[vuln.Affected] = true
		
		switch vuln.Severity {
		case DepSeverityCritical:
			summary.CriticalVulns++
		case DepSeverityHigh:
			summary.HighVulns++
		case DepSeverityMedium:
			summary.MediumVulns++
		case DepSeverityLow:
			summary.LowVulns++
		case DepSeverityInfo:
			summary.InfoVulns++
		}
	}
	
	summary.VulnerableDeps = len(vulnDeps)
	
	// Calculate security score (0-100)
	score := 100.0
	score -= float64(summary.CriticalVulns) * 25
	score -= float64(summary.HighVulns) * 15
	score -= float64(summary.MediumVulns) * 5
	score -= float64(summary.LowVulns) * 2
	score -= float64(summary.InfoVulns) * 0.5
	
	if score < 0 {
		score = 0
	}
	summary.SecurityScore = score
	
	// Calculate compliance score (0-100)
	summary.ComplianceScore = score // Simplified
	
	return summary
}

// generateRecommendations generates security recommendations
func (ds *DependencyScanner) generateRecommendations(vulnerabilities []Vulnerability) []string {
	recs := make([]string, 0)
	
	if len(vulnerabilities) > 0 {
		recs = append(recs, "Update vulnerable dependencies to fixed versions")
	}
	
	// Check for critical vulnerabilities
	for _, vuln := range vulnerabilities {
		if vuln.Severity == DepSeverityCritical {
			recs = append(recs, fmt.Sprintf("URGENT: Address critical vulnerability %s", vuln.ID))
		}
	}
	
	if len(recs) == 0 {
		recs = append(recs, "No security vulnerabilities found")
		recs = append(recs, "Continue regular dependency updates")
	}
	
	return recs
}

// initializeVulnDB initializes vulnerability database
func (ds *DependencyScanner) initializeVulnDB() {
	ds.mutex.Lock()
	defer ds.mutex.Unlock()
	
	// Sample vulnerability database
	// In production, load from CVE database
	ds.vulnerabilityDB["golang.org/x/crypto"] = []Vulnerability{
		{
			ID:           "GO-2023-1234",
			Severity:     DepSeverityHigh,
			CVE:          "CVE-2023-39325",
			Title:        "HTTP/2 Request Smuggling",
			Description:  "Request smuggling vulnerability in HTTP/2 server",
			Published:    time.Date(2023, 8, 30, 0, 0, 0, 0, time.UTC),
			Updated:      time.Date(2023, 9, 1, 0, 0, 0, 0, time.UTC),
			Affected:     "<0.14.0",
			Fixed:        "0.14.0",
			Score:        7.5,
			AttackVector: "network",
			Impact:       "privilege escalation",
		},
	}
	
	ds.vulnerabilityDB["github.com/gin-gonic/gin"] = []Vulnerability{
		{
			ID:           "GHSA-237h-m222-6432",
			Severity:     DepSeverityCritical,
			CVE:          "CVE-2023-36084",
			Title:        "Memory exhaustion in multipart form",
			Description:  "Potential DoS via memory exhaustion",
			Published:    time.Date(2023, 10, 15, 0, 0, 0, 0, time.UTC),
			Affected:     "<1.9.1",
			Fixed:        "1.9.1",
			Score:        9.8,
			AttackVector: "network",
			Impact:       "denial of service",
		},
	}
}

// GenerateSBOM generates Software Bill of Materials
func (dm *DependencyManager) GenerateSBOM(component string) (*SBOM, error) {
	// In production, use proper SBOM generation tools
	// This is a simplified implementation
	
	dependencies := dm.collectDependencies(component)
	
	sbom := &SBOM{
		Version:     "1.0",
		Generated:   time.Now(),
		Component:   component,
		Dependencies: dependencies,
		Metadata: map[string]interface{}{
			"generator": "vaughan-crush",
			"format":    "cyclonedx",
		},
	}
	
	// Calculate SBOM hash
	sbom.Hash = dm.calculateSBOMHash(sbom)
	
	return sbom, nil
}

// collectDependencies collects all dependencies
func (dm *DependencyManager) collectDependencies(component string) []Dependency {
	// In production, scan go.mod and other dependency files
	// This is a simplified implementation
	
	return []Dependency{
		{
			Name:    "github.com/r4v3n/vaughan-cli",
			Version: "1.0.0",
			Source:  "go.mod",
			License: "MIT",
		},
		{
			Name:    "github.com/gin-gonic/gin",
			Version: "1.9.1",
			Source:  "go.mod",
			License: "MIT",
		},
		{
			Name:    "golang.org/x/crypto",
			Version: "0.14.0",
			Source:  "go.mod",
			License: "BSD-3-Clause",
		},
	}
}

// calculateSBOMHash calculates hash for SBOM integrity
func (dm *DependencyManager) calculateSBOMHash(sbom *SBOM) string {
	data, _ := json.Marshal(sbom)
	hash := sha256.Sum256(data)
	return base64.StdEncoding.EncodeToString(hash[:])
}

// SaveSBOM saves SBOM to file
func (dm *DependencyManager) SaveSBOM(sbom *SBOM) error {
	// Ensure directory exists
	dir := filepath.Dir(dm.sbomPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create SBOM directory: %w", err)
	}
	
	// Save SBOM with integrity signature
	data, err := json.MarshalIndent(sbom, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal SBOM: %w", err)
	}
	
	// Add HMAC signature
	signature := dm.signSBOM(data)
	sbomData := append(data, []byte("\nHMAC:"+signature)...)
	
	return os.WriteFile(dm.sbomPath, sbomData, 0644)
}

// LoadSBOM loads SBOM from file
func (dm *DependencyManager) LoadSBOM() (*SBOM, error) {
	data, err := os.ReadFile(dm.sbomPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read SBOM: %w", err)
	}
	
	// Verify integrity
	if !dm.verifySBOM(data) {
		return nil, fmt.Errorf("SBOM integrity check failed")
	}
	
	// Remove signature
	data = bytes.TrimSuffix(data, []byte("\nHMAC:...")) // Simplified
	
	var sbom SBOM
	if err := json.Unmarshal(data, &sbom); err != nil {
		return nil, fmt.Errorf("failed to unmarshal SBOM: %w", err)
	}
	
	return &sbom, nil
}

// signSBOM creates HMAC signature for SBOM
func (dm *DependencyManager) signSBOM(data []byte) string {
	key := []byte("vaughan-crush-sbom-key") // In production, use secure key
	hmac := hmac.New(sha256.New, key)
	hmac.Write(data)
	return base64.StdEncoding.EncodeToString(hmac.Sum(nil))
}

// verifySBOM verifies SBOM integrity
func (dm *DependencyManager) verifySBOM(data []byte) bool {
	// Simplified verification
	// In production, implement proper HMAC verification
	return true
}

// AutoUpdateDependencies automatically updates vulnerable dependencies
func (dm *DependencyManager) AutoUpdateDependencies() error {
	if !dm.autoUpdate {
		return fmt.Errorf("auto-update is disabled")
	}
	
	// Load current SBOM
	sbom, err := dm.LoadSBOM()
	if err != nil {
		return fmt.Errorf("failed to load SBOM: %w", err)
	}
	
	// Scan for vulnerabilities
	result, err := dm.scanner.ScanDependencies(sbom.Dependencies)
	if err != nil {
		return fmt.Errorf("failed to scan dependencies: %w", err)
	}
	
	// Auto-update critical and high vulnerabilities
	for _, vuln := range result.Vulnerabilities {
		if vuln.Severity == DepSeverityCritical || vuln.Severity == DepSeverityHigh {
			if vuln.Fixed != "" {
				if err := dm.updateDependency(vuln.Affected, vuln.Fixed); err != nil {
					dm.logger.LogDependencyUpdate(vuln.Affected, vuln.Fixed, false, err.Error())
				} else {
					dm.logger.LogDependencyUpdate(vuln.Affected, vuln.Fixed, true, "")
				}
			}
		}
	}
	
	// Generate updated SBOM
	newSBOM, err := dm.GenerateSBOM(sbom.Component)
	if err != nil {
		return fmt.Errorf("failed to generate updated SBOM: %w", err)
	}
	
	return dm.SaveSBOM(newSBOM)
}

// updateDependency updates a specific dependency
func (dm *DependencyManager) updateDependency(name, version string) error {
	// In production, use proper package management
	// This is a simplified implementation
	
	// Example: run go get package@version
	// cmd := exec.Command("go", "get", fmt.Sprintf("%s@%s", name, version))
	// return cmd.Run()
	
	return nil
}

// CheckLicenseCompliance checks license compliance
func (dm *DependencyManager) CheckLicenseCompliance(dependencies []Dependency) ([]string, error) {
	var issues []string
	
	// Allowed licenses
	allowedLicenses := map[string]bool{
		"MIT":           true,
		"BSD-3-Clause": true,
		"BSD-2-Clause": true,
		"Apache-2.0":   true,
		"ISC":           true,
	}
	
	for _, dep := range dependencies {
		if !allowedLicenses[dep.License] {
			issues = append(issues, fmt.Sprintf("License issue: %s uses %s", dep.Name, dep.License))
		}
	}
	
	return issues, nil
}

// GetDependencyVulnerabilities returns vulnerabilities for a specific dependency
func (ds *DependencyScanner) GetDependencyVulnerabilities(name, version string) []Vulnerability {
	dep := Dependency{Name: name, Version: version}
	return ds.scanDependency(dep)
}

// LogDependencyScan logs dependency scan results
func (sl *SecurityLogger) LogDependencyScan(result *ScanResult) {
	event := SecurityEvent{
		Type:        "dependency_scan",
		Severity:    SeverityInfo,
		Description: "Dependency vulnerability scan completed",
		Details: map[string]interface{}{
			"total_deps":        result.Summary.TotalDeps,
			"vulnerable_deps":   result.Summary.VulnerableDeps,
			"critical_vulns":    result.Summary.CriticalVulns,
			"high_vulns":        result.Summary.HighVulns,
			"security_score":    result.Summary.SecurityScore,
			"compliance_score":  result.Summary.ComplianceScore,
			"scan_duration":     result.Duration.String(),
		},
	}
	
	// Adjust severity based on findings
	if result.Summary.CriticalVulns > 0 {
		event.Severity = SeverityCritical
	} else if result.Summary.HighVulns > 0 {
		event.Severity = SeverityHigh
	} else if result.Summary.VulnerableDeps > 0 {
		event.Severity = SeverityMedium
	}
	
	sl.LogEvent(event)
}

// LogDependencyUpdate logs dependency update attempts
func (sl *SecurityLogger) LogDependencyUpdate(name, version string, success bool, errMsg string) {
	event := SecurityEvent{
		Type:        "dependency_update",
		Severity:    SeverityInfo,
		Description: fmt.Sprintf("Dependency update: %s@%s", name, version),
		Details: map[string]interface{}{
			"name":    name,
			"version": version,
			"success": success,
			"error":   errMsg,
		},
	}
	
	if !success {
		event.Severity = SeverityHigh
	}
	
	sl.LogEvent(event)
}