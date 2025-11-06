package update

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/r4v3n/vaughan-cli/internal/errors"
	"github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// Manager implements UpdateManager interface
type Manager struct {
	currentVersion string
	updateURL     string
	configPath    string
}

// ReleaseInfo represents GitHub release information
type ReleaseInfo struct {
	TagName    string `json:"tag_name"`
	Name       string `json:"name"`
	Body       string `json:"body"`
	Draft      bool   `json:"draft"`
	Prerelease bool   `json:"prerelease"`
	PublishedAt string `json:"published_at"`
	Assets     []Asset `json:"assets"`
}

// Asset represents a release asset
type Asset struct {
	Name               string `json:"name"`
	ContentType        string `json:"content_type"`
	Size               int    `json:"size"`
	BrowserDownloadURL  string `json:"browser_download_url"`
}

// NewManager creates a new update manager
func NewManager(currentVersion string, configPath string) interfaces.UpdateManager {
	return &Manager{
		currentVersion: currentVersion,
		updateURL:     "https://api.github.com/repos/charmbracelet/crush/releases",
		configPath:    configPath,
	}
}

// CheckForUpdates implements UpdateManager interface
func (m *Manager) CheckForUpdates() (interfaces.UpdateInfo, error) {
	resp, err := http.Get(m.updateURL)
	if err != nil {
		return interfaces.UpdateInfo{}, errors.ErrNetworkUnavailable.WithCause(err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return interfaces.UpdateInfo{}, fmt.Errorf("failed to fetch releases: %s", resp.Status)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return interfaces.UpdateInfo{}, errors.ErrNetworkUnavailable.WithCause(err)
	}

	var releases []ReleaseInfo
	if err := json.Unmarshal(body, &releases); err != nil {
		return interfaces.UpdateInfo{}, fmt.Errorf("failed to parse releases: %w", err)
	}

	if len(releases) == 0 {
		return interfaces.UpdateInfo{}, nil
	}

	// Get latest stable release
	var latestRelease *ReleaseInfo
	for _, release := range releases {
		if release.Draft || release.Prerelease {
			continue
		}
		if latestRelease == nil || release.TagName > latestRelease.TagName {
			latestRelease = &release
		}
	}

	if latestRelease == nil {
		return interfaces.UpdateInfo{}, nil
	}

	// Check if update is available
	if m.isNewerVersion(latestRelease.TagName, m.currentVersion) {
		return interfaces.UpdateInfo{
			Version:      latestRelease.TagName,
			ReleaseNotes: latestRelease.Body,
			DownloadURL:  m.getDownloadURL(latestRelease),
			Critical:     m.isCriticalUpdate(latestRelease),
		}, nil
	}

	return interfaces.UpdateInfo{}, nil
}

// ApplyUpdate implements UpdateManager interface
func (m *Manager) ApplyUpdate(updateInfo interfaces.UpdateInfo) error {
	// Download the update
	resp, err := http.Get(updateInfo.DownloadURL)
	if err != nil {
		return errors.ErrNetworkUnavailable.WithCause(err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("failed to download update: %s", resp.Status)
	}

	// Create temporary file
	tempFile, err := os.CreateTemp("", "vaughan-crush-update-*")
	if err != nil {
		return errors.ErrFilePermission.WithCause(err)
	}
	defer os.Remove(tempFile.Name())

	// Write update to temporary file
	_, err = io.Copy(tempFile, resp.Body)
	if err != nil {
		return errors.ErrFilePermission.WithCause(err)
	}
	tempFile.Close()

	// Make executable
	if err := os.Chmod(tempFile.Name(), 0755); err != nil {
		return errors.ErrFilePermission.WithCause(err)
	}

	// Replace current binary
	currentBinary, err := os.Executable()
	if err != nil {
		return errors.ErrFileNotFound.WithCause(err)
	}

	if err := os.Rename(tempFile.Name(), currentBinary); err != nil {
		// On Windows, we can't replace running binary
		if runtime.GOOS == "windows" {
			// Create a batch file to replace after restart
			return m.createWindowsUpdater(tempFile.Name(), currentBinary)
		}
		return errors.ErrFilePermission.WithCause(err)
	}

	return nil
}

// GetCurrentVersion implements UpdateManager interface
func (m *Manager) GetCurrentVersion() string {
	return m.currentVersion
}

// IsUpdateAvailable implements UpdateManager interface
func (m *Manager) IsUpdateAvailable() bool {
	updateInfo, err := m.CheckForUpdates()
	if err != nil {
		return false
	}
	return updateInfo.Version != ""
}

// isNewerVersion checks if newVersion is newer than currentVersion
func (m *Manager) isNewerVersion(newVersion, currentVersion string) bool {
	// Simple version comparison (v1.2.3 vs v1.2.4)
	newClean := cleanVersion(newVersion)
	currentClean := cleanVersion(currentVersion)

	return newClean > currentClean
}

// cleanVersion removes 'v' prefix and returns comparable version
func cleanVersion(version string) string {
	if len(version) > 0 && version[0] == 'v' {
		version = version[1:]
	}
	return version
}

// getDownloadURL returns appropriate download URL for the current platform
func (m *Manager) getDownloadURL(release *ReleaseInfo) string {
	platform := m.getPlatformTag()
	
	for _, asset := range release.Assets {
		if m.isAssetForPlatform(asset.Name, platform) {
			return asset.BrowserDownloadURL
		}
	}
	
	return "" // No suitable asset found
}

// getPlatformTag returns platform tag for current system
func (m *Manager) getPlatformTag() string {
	os := runtime.GOOS
	arch := runtime.GOARCH
	
	switch os {
	case "darwin":
		if arch == "arm64" {
			return "darwin-arm64"
		}
		return "darwin-amd64"
	case "linux":
		if arch == "arm64" {
			return "linux-arm64"
		}
		return "linux-amd64"
	case "windows":
		if arch == "arm64" {
			return "windows-arm64"
		}
		return "windows-amd64"
	default:
		return fmt.Sprintf("%s-%s", os, arch)
	}
}

// isAssetForPlatform checks if asset name matches platform
func (m *Manager) isAssetForPlatform(assetName, platform string) bool {
	return assetName == fmt.Sprintf("crush-%s", platform) ||
		assetName == fmt.Sprintf("crush-%s.exe", platform) ||
		assetName == fmt.Sprintf("crush-%s.tar.gz", platform)
}

// isCriticalUpdate checks if release contains critical security fixes
func (m *Manager) isCriticalUpdate(release *ReleaseInfo) bool {
	// Check release notes for critical keywords
	notes := strings.ToLower(release.Body)
	return strings.Contains(notes, "critical") ||
		strings.Contains(notes, "security") ||
		strings.Contains(notes, "vulnerability")
}

// createWindowsUpdater creates a batch file to update on Windows restart
func (m *Manager) createWindowsUpdater(tempFile, currentBinary string) error {
	batchContent := fmt.Sprintf(`@echo off
timeout /t 2 /nobreak
move /Y "%s" "%s"
del "%s"
echo Update completed successfully!
`, tempFile, currentBinary, tempFile)

	batchFile := filepath.Join(filepath.Dir(currentBinary), "update.bat")
	
	file, err := os.Create(batchFile)
	if err != nil {
		return err
	}
	defer file.Close()

	_, err = file.WriteString(batchContent)
	if err != nil {
		return err
	}

	return nil
}

// GetConfigPath returns current configuration path
func (m *Manager) GetConfigPath() string {
	return m.configPath
}

// BackupConfig creates a backup of current configuration before update
func (m *Manager) BackupConfig() error {
	if m.configPath == "" {
		return nil
	}

	backupPath := fmt.Sprintf("%s.backup.%d", m.configPath, time.Now().Unix())
	return os.Rename(m.configPath, backupPath)
}

// RestoreConfig restores configuration from backup
func (m *Manager) RestoreConfig(backupPath string) error {
	return os.Rename(backupPath, m.configPath)
}