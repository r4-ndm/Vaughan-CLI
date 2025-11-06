package security

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// FileSecurity manages file system access controls
type FileSecurity struct {
	policy       *FileSystemPolicy
	accessLog    map[string]*FileAccess
	mutex        sync.RWMutex
	logger       *SecurityLogger
	allowedPaths map[string]bool
}

// FileSystemPolicy defines file system access rules
type FileSystemPolicy struct {
	AllowedDirectories   []string          `json:"allowed_directories"`
	BlockedDirectories   []string          `json:"blocked_directories"`
	AllowedExtensions     []string          `json:"allowed_extensions"`
	BlockedExtensions     []string          `json:"blocked_extensions"`
	MaxFileSize          int64             `json:"max_file_size"`
	AllowedOperations     []FileOperation   `json:"allowed_operations"`
	BlockedOperations     []FileOperation   `json:"blocked_operations"`
	TempDir             string            `json:"temp_dir"`
	RequireExplicitPath   bool              `json:"require_explicit_path"`
	CheckFileIntegrity    bool              `json:"check_file_integrity"`
	ScanForMalware       bool              `json:"scan_for_malware"`
	AuditFileAccess       bool              `json:"audit_file_access"`
}

// FileOperation represents file system operations
type FileOperation string

const (
	FileOpRead    FileOperation = "read"
	FileOpWrite   FileOperation = "write"
	FileOpCreate  FileOperation = "create"
	FileOpDelete  FileOperation = "delete"
	FileOpExecute FileOperation = "execute"
	FileOpRename  FileOperation = "rename"
	FileOpCopy    FileOperation = "copy"
	FileOpMove    FileOperation = "move"
)

// FileAccess represents file access information
type FileAccess struct {
	Path         string        `json:"path"`
	Operation    FileOperation `json:"operation"`
	UserID       string        `json:"user_id"`
	SessionID    string        `json:"session_id"`
	Timestamp    time.Time     `json:"timestamp"`
	Success      bool          `json:"success"`
	Size         int64         `json:"size,omitempty"`
	Hash         string        `json:"hash,omitempty"`
	Error        string        `json:"error,omitempty"`
	RemoteAddr   string        `json:"remote_addr,omitempty"`
}

// SecureFile represents a file with security metadata
type SecureFile struct {
	Path         string            `json:"path"`
	Name         string            `json:"name"`
	Size         int64             `json:"size"`
	Hash         string            `json:"hash"`
	CreatedAt    time.Time         `json:"created_at"`
	ModifiedAt   time.Time         `json:"modified_at"`
	AccessedAt   time.Time         `json:"accessed_at"`
	Permissions  string            `json:"permissions"`
	Owner        string            `json:"owner"`
	Metadata     map[string]string `json:"metadata"`
}

// NewFileSecurity creates a file security manager
func NewFileSecurity(policy *FileSystemPolicy, logger *SecurityLogger) *FileSecurity {
	fs := &FileSecurity{
		policy:       policy,
		accessLog:    make(map[string]*FileAccess),
		logger:       logger,
		allowedPaths: make(map[string]bool),
	}
	
	// Initialize allowed paths
	for _, dir := range policy.AllowedDirectories {
		fs.allowedPaths[dir] = true
	}
	
	// Ensure temp directory exists
	if policy.TempDir != "" {
		os.MkdirAll(policy.TempDir, 0700)
	}
	
	return fs
}

// ValidatePath checks if file path is allowed
func (fs *FileSecurity) ValidatePath(path string, operation FileOperation, ctx *Context) error {
	// Clean and resolve path
	cleanPath := filepath.Clean(path)
	
	// Check for path traversal
	if strings.Contains(cleanPath, "..") {
		return fmt.Errorf("path traversal detected: %s", path)
	}
	
	// Convert to absolute path for comparison
	absPath, err := filepath.Abs(cleanPath)
	if err != nil {
		return fmt.Errorf("failed to resolve absolute path: %w", err)
	}
	
	// Check blocked directories
	for _, blocked := range fs.policy.BlockedDirectories {
		if strings.HasPrefix(absPath, blocked) {
			return fmt.Errorf("access to blocked directory denied: %s", blocked)
		}
	}
	
	// Check allowed directories
	if len(fs.policy.AllowedDirectories) > 0 {
		allowed := false
		for _, allowed := range fs.policy.AllowedDirectories {
			if strings.HasPrefix(absPath, allowed) {
				allowed = true
				break
			}
		}
		if !allowed {
			return fmt.Errorf("path not in allowed directories: %s", absPath)
		}
	}
	
	// Check file extension restrictions
	if err := fs.checkFileExtension(absPath); err != nil {
		return err
	}
	
	// Check operation permissions
	if err := fs.checkOperation(operation); err != nil {
		return err
	}
	
	// Log file access attempt
	if fs.policy.AuditFileAccess {
		fs.logFileAccess(path, operation, ctx, nil, nil)
	}
	
	return nil
}

// SecureRead reads file with security controls
func (fs *FileSecurity) SecureRead(path string, ctx *Context) ([]byte, error) {
	// Validate access
	if err := fs.ValidatePath(path, FileOpRead, ctx); err != nil {
		fs.logFileAccess(path, FileOpRead, ctx, nil, err)
		return nil, err
	}
	
	// Check if file exists
	info, err := os.Stat(path)
	if err != nil {
		fs.logFileAccess(path, FileOpRead, ctx, nil, err)
		return nil, fmt.Errorf("file access denied: %w", err)
	}
	
	// Check file size
	if fs.policy.MaxFileSize > 0 && info.Size() > fs.policy.MaxFileSize {
		err := fmt.Errorf("file size exceeds limit: %d > %d", info.Size(), fs.policy.MaxFileSize)
		fs.logFileAccess(path, FileOpRead, ctx, nil, err)
		return nil, err
	}
	
	// Read file
	data, err := os.ReadFile(path)
	if err != nil {
		fs.logFileAccess(path, FileOpRead, ctx, nil, err)
		return nil, fmt.Errorf("failed to read file: %w", err)
	}
	
	// Check file integrity if required
	if fs.policy.CheckFileIntegrity {
		hash := fs.calculateHash(data)
		fs.logFileAccess(path, FileOpRead, ctx, &hash, nil)
	} else {
		fs.logFileAccess(path, FileOpRead, ctx, nil, nil)
	}
	
	return data, nil
}

// SecureWrite writes file with security controls
func (fs *FileSecurity) SecureWrite(path string, data []byte, ctx *Context) error {
	// Validate access
	if err := fs.ValidatePath(path, FileOpWrite, ctx); err != nil {
		fs.logFileAccess(path, FileOpWrite, ctx, nil, err)
		return err
	}
	
	// Check file size
	if fs.policy.MaxFileSize > 0 && int64(len(data)) > fs.policy.MaxFileSize {
		err := fmt.Errorf("file size exceeds limit: %d > %d", len(data), fs.policy.MaxFileSize)
		fs.logFileAccess(path, FileOpWrite, ctx, nil, err)
		return err
	}
	
	// Scan for malware if required
	if fs.policy.ScanForMalware {
		if err := fs.scanForMalware(data); err != nil {
			fs.logFileAccess(path, FileOpWrite, ctx, nil, err)
			return err
		}
	}
	
	// Ensure directory exists
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		fs.logFileAccess(path, FileOpWrite, ctx, nil, err)
		return fmt.Errorf("failed to create directory: %w", err)
	}
	
	// Write file with secure permissions
	err := os.WriteFile(path, data, 0600) // rw-------
	if err != nil {
		fs.logFileAccess(path, FileOpWrite, ctx, nil, err)
		return fmt.Errorf("failed to write file: %w", err)
	}
	
	// Log successful write
	hash := fs.calculateHash(data)
	fs.logFileAccess(path, FileOpWrite, ctx, &hash, nil)
	
	return nil
}

// SecureDelete deletes file with security controls
func (fs *FileSecurity) SecureDelete(path string, ctx *Context) error {
	// Validate access
	if err := fs.ValidatePath(path, FileOpDelete, ctx); err != nil {
		fs.logFileAccess(path, FileOpDelete, ctx, nil, err)
		return err
	}
	
	// Check if file exists
	if _, err := os.Stat(path); err != nil {
		fs.logFileAccess(path, FileOpDelete, ctx, nil, err)
		return fmt.Errorf("file not found: %w", err)
	}
	
	// Delete file
	err := os.Remove(path)
	if err != nil {
		fs.logFileAccess(path, FileOpDelete, ctx, nil, err)
		return fmt.Errorf("failed to delete file: %w", err)
	}
	
	// Log successful deletion
	fs.logFileAccess(path, FileOpDelete, ctx, nil, nil)
	
	return nil
}

// CreateSecureTempFile creates a secure temporary file
func (fs *FileSecurity) CreateSecureTempFile(prefix string, ctx *Context) (string, error) {
	if fs.policy.TempDir == "" {
		return "", fmt.Errorf("temporary directory not configured")
	}
	
	// Create temp file with secure permissions
	tempFile, err := os.CreateTemp(fs.policy.TempDir, prefix+"_")
	if err != nil {
		fs.logFileAccess(tempFile.Name(), FileOpCreate, ctx, nil, err)
		return "", fmt.Errorf("failed to create temp file: %w", err)
	}
	
	// Set secure permissions
	tempFile.Chmod(0600)
	tempFile.Close()
	
	// Log temp file creation
	fs.logFileAccess(tempFile.Name(), FileOpCreate, ctx, nil, nil)
	
	return tempFile.Name(), nil
}

// ListSecureFiles lists files with security controls
func (fs *FileSecurity) ListSecureFiles(directory string, ctx *Context) ([]*SecureFile, error) {
	// Validate directory access
	if err := fs.ValidatePath(directory, FileOpRead, ctx); err != nil {
		return nil, err
	}
	
	// Read directory
	entries, err := os.ReadDir(directory)
	if err != nil {
		return nil, fmt.Errorf("failed to read directory: %w", err)
	}
	
	var files []*SecureFile
	for _, entry := range entries {
		if entry.IsDir() {
			continue // Skip directories for now
		}
		
		info, err := entry.Info()
		if err != nil {
			continue // Skip files that can't be accessed
		}
		
		filePath := filepath.Join(directory, entry.Name())
		
		// Calculate hash for integrity
		data, err := os.ReadFile(filePath)
		hash := ""
		if err == nil {
			hash = fs.calculateHash(data)
		}
		
		file := &SecureFile{
			Path:       filePath,
			Name:       entry.Name(),
			Size:       info.Size(),
			Hash:       hash,
			CreatedAt:  info.ModTime(), // Use mod time as creation time
			ModifiedAt: info.ModTime(),
			AccessedAt: time.Now(),
			Permissions: fmt.Sprintf("%04o", info.Mode().Perm()),
			Metadata:   make(map[string]string),
		}
		
		files = append(files, file)
	}
	
	return files, nil
}

// checkFileExtension validates file extensions
func (fs *FileSecurity) checkFileExtension(path string) error {
	ext := strings.ToLower(filepath.Ext(path))
	
	// Check blocked extensions
	for _, blocked := range fs.policy.BlockedExtensions {
		if ext == blocked {
			return fmt.Errorf("file extension '%s' is blocked", ext)
		}
	}
	
	// Check allowed extensions (if specified)
	if len(fs.policy.AllowedExtensions) > 0 {
		allowed := false
		for _, allowed := range fs.policy.AllowedExtensions {
			if ext == allowed {
				allowed = true
				break
			}
		}
		if !allowed && ext != "" { // Allow no extension if not blocked
			return fmt.Errorf("file extension '%s' is not allowed", ext)
		}
	}
	
	return nil
}

// checkOperation validates file operations
func (fs *FileSecurity) checkOperation(operation FileOperation) error {
	// Check blocked operations
	for _, blocked := range fs.policy.BlockedOperations {
		if operation == blocked {
			return fmt.Errorf("file operation '%s' is blocked", operation)
		}
	}
	
	// Check allowed operations (if specified)
	if len(fs.policy.AllowedOperations) > 0 {
		allowed := false
		for _, allowed := range fs.policy.AllowedOperations {
			if operation == allowed {
				allowed = true
				break
			}
		}
		if !allowed {
			return fmt.Errorf("file operation '%s' is not allowed", operation)
		}
	}
	
	return nil
}

// calculateHash calculates file hash for integrity
func (fs *FileSecurity) calculateHash(data []byte) string {
	// Simplified hash calculation
	// In production, use SHA-256 or other secure hash
	hash := 0
	for _, b := range data {
		hash = hash*31 + int(b)
	}
	return fmt.Sprintf("%x", hash)
}

// scanForMalware performs basic malware scanning
func (fs *FileSecurity) scanForMalware(data []byte) error {
	// Simplified malware detection
	// In production, use real antivirus scanning
	malwarePatterns := []string{
		"eval(",
		"system(",
		"exec(",
		"shell_exec(",
		"passthru(",
		"<script>",
		"javascript:",
	}
	
	dataStr := string(data)
	for _, pattern := range malwarePatterns {
		if strings.Contains(strings.ToLower(dataStr), strings.ToLower(pattern)) {
			return fmt.Errorf("potential malware detected: pattern '%s'", pattern)
		}
	}
	
	return nil
}

// logFileAccess logs file access events
func (fs *FileSecurity) logFileAccess(path string, operation FileOperation, ctx *Context, hash *string, err error) {
	if !fs.policy.AuditFileAccess {
		return
	}
	
	access := &FileAccess{
		Path:      path,
		Operation: operation,
		UserID:    ctx.UserID,
		SessionID: ctx.SessionID,
		Timestamp: time.Now(),
		Success:   err == nil,
		Error:     "",
	}
	
	if hash != nil {
		access.Hash = *hash
	}
	
	if err != nil {
		access.Error = err.Error()
	}
	
	// Get file size
	if info, statErr := os.Stat(path); statErr == nil {
		access.Size = info.Size()
	}
	
	// Log to security system
	if fs.logger != nil {
		eventType := EventFileAccess
		severity := SeverityMedium
		
		if err != nil {
			eventType = EventFileBlocked
			severity = SeverityHigh
		}
		
		event := SecurityEvent{
			Type:        eventType,
			Severity:    severity,
			UserID:      ctx.UserID,
			SessionID:   ctx.SessionID,
			Description: fmt.Sprintf("File %s: %s", operation, path),
			Details: map[string]interface{}{
				"path":      path,
				"operation": string(operation),
				"size":      access.Size,
				"hash":      access.Hash,
				"error":     access.Error,
			},
		}
		
		fs.logger.LogEvent(event)
	}
	
	// Add to internal access log
	fs.mutex.Lock()
	fs.accessLog[access.Path+"_"+string(operation)] = access
	fs.mutex.Unlock()
}

// GetAccessLog returns file access logs
func (fs *FileSecurity) GetAccessLog(limit int) []*FileAccess {
	fs.mutex.RLock()
	defer fs.mutex.RUnlock()
	
	logs := make([]*FileAccess, 0, len(fs.accessLog))
	for _, access := range fs.accessLog {
		logs = append(logs, access)
	}
	
	// Sort by timestamp (most recent first)
	// In production, implement proper sorting
	
	if limit > 0 && len(logs) > limit {
		logs = logs[:limit]
	}
	
	return logs
}

// CleanupAccessLog removes old access logs
func (fs *FileSecurity) CleanupAccessLog(olderThan time.Duration) int {
	fs.mutex.Lock()
	defer fs.mutex.Unlock()
	
	cutoff := time.Now().Add(-olderThan)
	removed := 0
	
	for key, access := range fs.accessLog {
		if access.Timestamp.Before(cutoff) {
			delete(fs.accessLog, key)
			removed++
		}
	}
	
	return removed
}

// DefaultFileSystemPolicy returns a secure default file system policy
func DefaultFileSystemPolicy() *FileSystemPolicy {
	homeDir, _ := os.UserHomeDir()
	return &FileSystemPolicy{
		AllowedDirectories: []string{
			homeDir + "/Documents",
			homeDir + "/Downloads",
			homeDir + "/Desktop",
			"/tmp/vaughan-crush",
		},
		BlockedDirectories: []string{
			"/etc",
			"/var",
			"/usr",
			"/bin",
			"/sbin",
			"/root",
			"/sys",
			"/proc",
		},
		AllowedExtensions: []string{
			".txt", ".md", ".json", ".yaml", ".yml",
			".go", ".js", ".ts", ".py", ".sh",
			".pdf", ".doc", ".docx", ".xls", ".xlsx",
		},
		BlockedExtensions: []string{
			".exe", ".bat", ".cmd", ".scr", ".pif",
			".com", ".vbs", ".js", ".jar", ".app",
		},
		MaxFileSize:      100 * 1024 * 1024, // 100MB
		AllowedOperations: []FileOperation{FileOpRead, FileOpWrite, FileOpCreate, FileOpDelete},
		BlockedOperations: []FileOperation{FileOpExecute},
		TempDir:          "/tmp/vaughan-crush",
		CheckFileIntegrity: true,
		ScanForMalware:    true,
		AuditFileAccess:   true,
	}
}

// RestrictiveFileSystemPolicy returns a highly restrictive policy
func RestrictiveFileSystemPolicy() *FileSystemPolicy {
	homeDir, _ := os.UserHomeDir()
	return &FileSystemPolicy{
		AllowedDirectories: []string{
			homeDir + "/Vaughan-Crush",
		},
		BlockedDirectories: []string{
			"/etc", "/var", "/usr", "/bin", "/sbin",
			"/root", "/sys", "/proc", "/boot", "/dev",
		},
		AllowedExtensions: []string{
			".txt", ".md", ".json", ".yaml", ".yml",
		},
		BlockedExtensions: []string{
			".exe", ".bat", ".cmd", ".scr", ".pif",
			".com", ".vbs", ".js", ".jar", ".app",
			".py", ".sh", ".pl", ".rb", ".php",
		},
		MaxFileSize:      10 * 1024 * 1024, // 10MB
		AllowedOperations: []FileOperation{FileOpRead, FileOpWrite},
		BlockedOperations: []FileOperation{FileOpExecute, FileOpDelete, FileOpCreate},
		TempDir:          "/tmp/vaughan-crush",
		CheckFileIntegrity: true,
		ScanForMalware:    true,
		AuditFileAccess:   true,
	}
}