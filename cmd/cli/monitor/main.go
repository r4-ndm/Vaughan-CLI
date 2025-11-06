package main

import (
	"context"
	"fmt"
	"os"
	"sort"
	"time"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"github.com/spf13/cobra"
	"net/http"
	"vaughan-cli/internal/security/real"
	"vaughan-cli/internal/monitoring"
)

var (
	port      int
	metrics   bool
	status    bool
	export    string
	interval  time.Duration
	filter    string
	monitoringSystem *real.SecurityMonitoringManager
	config          *real.MonitoringConfig
)

func init() {
	// Initialize real monitoring configuration
	config = &real.MonitoringConfig{
		Prometheus: struct {
			Port              string `json:"port" yaml:"port"`
			MetricsPath       string `json:"metrics_path" yaml:"metrics_path"`
			RegistryName      string `json:"registry_name" yaml:"registry_name"`
		}{
			Port:              "9090",
			MetricsPath:       "/metrics",
			RegistryName:      "vaughan_monitoring",
		},
		Elasticsearch: struct {
			Addresses        []string `json:"addresses" yaml:"addresses"`
			Username         string    `json:"username" yaml:"username"`
			Password         string    `json:"password" yaml:"password"`
			Index            string    `json:"index" yaml:"index"`
			SecurityEventsIndex string `json:"security_events_index" yaml:"security_events_index"`
		}{
			Addresses:          []string{"http://localhost:9200"},
			Username:           "elastic",
			Password:           "changeme",
			Index:              "vaughan_logs",
			SecurityEventsIndex: "vaughan_monitoring_events",
		},
		Influxdb: struct {
			URL      string `json:"url" yaml:"url"`
			Token    string `json:"token" yaml:"token"`
			Org      string `json:"org" yaml:"org"`
			Bucket   string `json:"bucket" yaml:"bucket"`
		}{
			URL:    "http://localhost:8086",
			Token:  "test-token",
			Org:    "vaughan",
			Bucket: "monitoring_metrics",
		},
		Sentry: struct {
			DSN          string `json:"dsn" yaml:"dsn"`
			Environment  string `json:"environment" yaml:"environment"`
			Release      string `json:"release" yaml:"release"`
			SampleRate   float64 `json:"sample_rate" yaml:"sample_rate"`
		}{
			DSN:         "https://test-sentry-dsn",
			Environment: "production",
			Release:     "vaughan-cli@1.0.0",
			SampleRate:  1.0,
		},
		Segment: struct {
			WriteKey string `json:"write_key" yaml:"write_key"`
		}{
			WriteKey: "test-segment-write-key",
		},
		Logging: struct {
			Level      string `json:"level" yaml:"level"`
			Format     string `json:"format" yaml:"format"`
			Output     string `json:"output" yaml:"output"`
			SecurityEventsFile string `json:"security_events_file" yaml:"security_events_file"`
		}{
			Level:              "info",
			Format:             "json",
			Output:             "console",
			SecurityEventsFile: "/var/log/vaughan/monitoring_events.log",
		},
		Alerting: struct {
			Enabled   bool     `json:"enabled" yaml:"enabled"`
			Channels  []string `json:"channels" yaml:"channels"`
			Threshold float64  `json:"threshold" yaml:"threshold"`
		}{
			Enabled:   true,
			Channels:  []string{"email", "slack"},
			Threshold: 5.0,
		},
	}

	// Create real security monitoring manager
	monitoringSystem = real.NewSecurityMonitoringManager(config)

	// Initialize monitoring (non-blocking)
	go func() {
		if err := monitoringSystem.InitializeSecurityMonitoringManager(); err != nil {
			fmt.Printf("⚠️  Monitor command monitoring initialization failed: %v\n", err)
		} else {
			fmt.Println("✅ Monitor command security monitoring initialized")
		}
	}()

	rootCmd.AddCommand(monitorStartCmd)
	rootCmd.AddCommand(monitorStopCmd)
	rootCmd.AddCommand(monitorStatusCmd)
	rootCmd.AddCommand(monitorMetricsCmd)
	rootCmd.AddCommand(monitorExportCmd)
	rootCmd.AddCommand(monitorAlertsCmd)

	// Monitor start command flags
	monitorStartCmd.Flags().IntVarP(&port, "port", "p", 9090, "Port for monitoring server")
	monitorStartCmd.Flags().BoolVar(&metrics, "metrics", true, "Enable Prometheus metrics endpoint")

	// Monitor status command flags
	monitorStatusCmd.Flags().StringVar(&filter, "filter", "", "Filter status by component")

	// Monitor metrics command flags
	monitorMetricsCmd.Flags().DurationVar(&interval, "interval", 30*time.Second, "Metrics refresh interval")
	monitorMetricsCmd.Flags().StringVar(&export, "export", "", "Export metrics to file")

	// Monitor export command flags
	monitorExportCmd.Flags().StringVar(&export, "output", "", "Output file path")
	monitorExportCmd.Flags().StringVar(&filter, "filter", "", "Filter exported data")
}

var monitorStartCmd = &cobra.Command{
	Use:   "start",
	Short: "Start monitoring server",
	Long:  `Start the Vaughan CLI monitoring server with real-time metrics and observability.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		// Log monitoring start event
		startEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_server_started",
			Severity:  "info",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command": "monitor start",
				"port":    port,
				"metrics": metrics,
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(startEvent)
		monitoringSystem.RecordSecurityEvent("monitoring_server_started", "info", "monitor_command")

		fmt.Printf("🚀 Starting Vaughan CLI monitoring server\n")
		fmt.Printf("📊 Port: %d\n", port)

		// Start monitoring server
		monitoringEngine := monitoring.NewMonitoringEngine(context.Background())
		err := monitoringEngine.StartServer(port, metrics)
		if err != nil {
			// Log server start failure
			failEvent := &real.SecurityEvent{
				ID:        generateMonitorEventID(),
				Type:      "monitoring_server_start_failure",
				Severity:  "high",
				Source:    "monitor_command",
				UserID:    getCurrentUserID(),
				IPAddress: getClientIP(),
				UserAgent: getUserAgent(),
				Details: map[string]interface{}{
					"command": "monitor start",
					"port":    port,
					"reason":  err.Error(),
				},
				Timestamp: time.Now(),
			}
			
			monitoringSystem.LogSecurityEvent(failEvent)
			monitoringSystem.RecordSecurityEvent("monitoring_server_start_failure", "high", "monitor_command")
			monitoringSystem.CaptureSecurityError(err, map[string]interface{}{
				"command": "monitor start",
				"port":    port,
				"source":  "monitor_command",
			})
			
			return fmt.Errorf("failed to start monitoring server: %w", err)
		}

		// Log server start success
		successEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_server_running",
			Severity:  "low",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command": "monitor start",
				"port":    port,
				"status":  "running",
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(successEvent)
		monitoringSystem.RecordSecurityEvent("monitoring_server_running", "low", "monitor_command")

		if metrics {
			// Start Prometheus metrics endpoint
			go func() {
				http.Handle("/metrics", promhttp.Handler())
				fmt.Printf("📈 Prometheus metrics available at http://localhost:%d/metrics\n", port)
				if err := http.ListenAndServe(fmt.Sprintf(":%d", port), nil); err != nil {
					monitoringSystem.CaptureSecurityError(err, map[string]interface{}{
						"component": "prometheus_server",
						"port":     port,
					})
				}
			}()
		}

		fmt.Printf("✅ Monitoring server started on port %d\n", port)
		if metrics {
			fmt.Printf("📊 Metrics endpoint: http://localhost:%d/metrics\n", port)
		}
		fmt.Println("⏹️  Press Ctrl+C to stop")

		// Keep server running
		select {}
	},
}

var monitorStopCmd = &cobra.Command{
	Use:   "stop",
	Short: "Stop monitoring server",
	Long:  `Stop the Vaughan CLI monitoring server gracefully.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		// Log monitoring stop event
		stopEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_server_stopped",
			Severity:  "info",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command": "monitor stop",
				"reason":  "user_initiated",
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(stopEvent)
		monitoringSystem.RecordSecurityEvent("monitoring_server_stopped", "info", "monitor_command")

		// Stop monitoring server
		monitoringEngine := monitoring.NewMonitoringEngine(context.Background())
		err := monitoringEngine.StopServer()
		if err != nil {
			// Log server stop failure
			failEvent := &real.SecurityEvent{
				ID:        generateMonitorEventID(),
				Type:      "monitoring_server_stop_failure",
				Severity:  "medium",
				Source:    "monitor_command",
				UserID:    getCurrentUserID(),
				IPAddress: getClientIP(),
				UserAgent: getUserAgent(),
				Details: map[string]interface{}{
					"command": "monitor stop",
					"reason":  err.Error(),
				},
				Timestamp: time.Now(),
			}
			
			monitoringSystem.LogSecurityEvent(failEvent)
			monitoringSystem.RecordSecurityEvent("monitoring_server_stop_failure", "medium", "monitor_command")
			
			return fmt.Errorf("failed to stop monitoring server: %w", err)
		}

		fmt.Println("✅ Monitoring server stopped successfully")
		return nil
	},
}

var monitorStatusCmd = &cobra.Command{
	Use:   "status",
	Short: "Show monitoring system status",
	Long:  `Display current status of the Vaughan CLI monitoring system.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		// Log status check event
		statusEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_status_check",
			Severity:  "info",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command": "monitor status",
				"filter":  filter,
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(statusEvent)

		fmt.Println("📊 Vaughan CLI Monitoring Status")

		// Get security monitoring status
		status := monitoringSystem.GetMonitoringStatus()
		fmt.Printf("\n🔒 Security Monitoring Status:\n")
		fmt.Printf("   Initialized: %v\n", status.Initialized)
		fmt.Printf("   Prometheus Enabled: %v\n", status.PrometheusEnabled)
		fmt.Printf("   Elasticsearch Enabled: %v\n", status.ElasticsearchEnabled)
		fmt.Printf("   InfluxDB Enabled: %v\n", status.InfluxDBEnabled)
		fmt.Printf("   Sentry Enabled: %v\n", status.SentryEnabled)
		fmt.Printf("   Segment Enabled: %v\n", status.SegmentEnabled)
		fmt.Printf("   Logging Enabled: %v\n", status.LoggingEnabled)
		fmt.Printf("   Last Check: %v\n", status.LastCheck)

		// Get security monitoring metrics
		metrics := monitoringSystem.GetMonitoringMetrics()
		fmt.Printf("\n📈 Security Monitoring Metrics:\n")
		fmt.Printf("   Security Events Logged: %d\n", metrics.SecurityEventsLogged)
		fmt.Printf("   Metrics Collected: %d\n", metrics.MetricsCollected)
		fmt.Printf("   Log Entries Created: %d\n", metrics.LogEntriesCreated)
		fmt.Printf("   Errors Captured: %d\n", metrics.ErrorsCaptured)
		fmt.Printf("   Alerts Triggered: %d\n", metrics.AlertsTriggered)
		fmt.Printf("   Elasticsearch Documents: %d\n", metrics.ElasticsearchDocuments)
		fmt.Printf("   InfluxDB Points: %d\n", metrics.InfluxDBPoints)
		fmt.Printf("   Segment Events: %d\n", metrics.SegmentEvents)
		fmt.Printf("   Sentry Events: %d\n", metrics.SentryEvents)
		fmt.Printf("   Last Updated: %v\n", metrics.LastUpdated)

		// Get system monitoring status
		monitoringEngine := monitoring.NewMonitoringEngine(context.Background())
		systemStatus, err := monitoringEngine.GetSystemStatus(filter)
		if err != nil {
			return fmt.Errorf("failed to get system status: %w", err)
		}

		fmt.Printf("\n💻 System Monitoring Status:\n")
		fmt.Printf("   CPU Usage: %.2f%%\n", systemStatus.CPUUsage)
		fmt.Printf("   Memory Usage: %.2f%%\n", systemStatus.MemoryUsage)
		fmt.Printf("   Disk Usage: %.2f%%\n", systemStatus.DiskUsage)
		fmt.Printf("   Network I/O: %s\n", systemStatus.NetworkIO)
		fmt.Printf("   Uptime: %s\n", systemStatus.Uptime)
		fmt.Printf("   Process Count: %d\n", systemStatus.ProcessCount)

		// Log status completion
		completionEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_status_completed",
			Severity:  "info",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command": "monitor status",
				"components_checked": "security,system",
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(completionEvent)

		return nil
	},
}

var monitorMetricsCmd = &cobra.Command{
	Use:   "metrics",
	Short: "Display monitoring metrics",
	Long:  `Display real-time monitoring metrics from the Vaughan CLI system.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		// Log metrics check event
		metricsEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_metrics_checked",
			Severity:  "info",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command":  "monitor metrics",
				"interval": interval.String(),
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(metricsEvent)

		fmt.Printf("📈 Vaughan CLI Real-Time Metrics\n")
		fmt.Printf("⏰ Refresh Interval: %s\n\n", interval)

		// Get monitoring engine
		monitoringEngine := monitoring.NewMonitoringEngine(context.Background())

		// Continuous metrics display
		ticker := time.NewTicker(interval)
		defer ticker.Stop()

		for {
			select {
			case <-ticker.C:
				// Clear screen
				fmt.Print("\033[H\033[2J")

				// Get current metrics
				currentMetrics, err := monitoringEngine.GetCurrentMetrics()
				if err != nil {
					monitoringSystem.CaptureSecurityError(err, map[string]interface{}{
						"command": "monitor metrics",
						"source": "monitor_command",
					})
					continue
				}

				// Display timestamp
				fmt.Printf("📊 Vaughan CLI Metrics - %s\n\n", time.Now().Format(time.RFC3339))

				// Display security metrics
				fmt.Printf("🔒 Security Metrics:\n")
				securityMetrics := monitoringSystem.GetMonitoringMetrics()
				fmt.Printf("   Security Events: %d\n", securityMetrics.SecurityEventsLogged)
				fmt.Printf("   Authentication Events: %d\n", currentMetrics.AuthenticationEvents)
				fmt.Printf("   Authorization Events: %d\n", currentMetrics.AuthorizationEvents)
				fmt.Printf("   Session Count: %d\n", currentMetrics.ActiveSessions)
				fmt.Printf("   Failed Logins: %d\n", currentMetrics.FailedLogins)
				fmt.Printf("   Rate Limits: %d\n\n", currentMetrics.RateLimits)

				// Display system metrics
				fmt.Printf("💻 System Metrics:\n")
				fmt.Printf("   CPU Usage: %.2f%%\n", currentMetrics.CPUUsage)
				fmt.Printf("   Memory Usage: %.2f%%\n", currentMetrics.MemoryUsage)
				fmt.Printf("   Disk Usage: %.2f%%\n", currentMetrics.DiskUsage)
				fmt.Printf("   Network In: %s\n", currentMetrics.NetworkIn)
				fmt.Printf("   Network Out: %s\n", currentMetrics.NetworkOut)
				fmt.Printf("   Request Rate: %.2f/s\n\n", currentMetrics.RequestRate)

				// Display application metrics
				fmt.Printf("🚀 Application Metrics:\n")
				fmt.Printf("   Total Requests: %d\n", currentMetrics.TotalRequests)
				fmt.Printf("   Error Rate: %.2f%%\n", currentMetrics.ErrorRate)
				fmt.Printf("   Response Time: %.2fms\n", currentMetrics.AverageResponseTime)
				fmt.Printf("   Active Connections: %d\n", currentMetrics.ActiveConnections)
				fmt.Printf("   Queue Length: %d\n\n", currentMetrics.QueueLength)

				// Export metrics if requested
				if export != "" {
					exportMetrics(export, currentMetrics, securityMetrics)
				}
			}
		}
	},
}

var monitorExportCmd = &cobra.Command{
	Use:   "export",
	Short: "Export monitoring data",
	Long:  `Export monitoring data to various formats for analysis and backup.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		if export == "" {
			return fmt.Errorf("output file is required for export")
		}

		// Log export event
		exportEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_data_exported",
			Severity:  "info",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command": "monitor export",
				"output":  export,
				"filter":  filter,
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(exportEvent)

		fmt.Printf("📤 Exporting monitoring data to: %s\n", export)

		// Get monitoring engine
		monitoringEngine := monitoring.NewMonitoringEngine(context.Background())

		// Export data
		exportData, err := monitoringEngine.ExportMonitoringData(filter)
		if err != nil {
			// Log export failure
			failEvent := &real.SecurityEvent{
				ID:        generateMonitorEventID(),
				Type:      "monitoring_data_export_failure",
				Severity:  "high",
				Source:    "monitor_command",
				UserID:    getCurrentUserID(),
				IPAddress: getClientIP(),
				UserAgent: getUserAgent(),
				Details: map[string]interface{}{
					"command": "monitor export",
					"output":  export,
					"reason":  err.Error(),
				},
				Timestamp: time.Now(),
			}
			
			monitoringSystem.LogSecurityEvent(failEvent)
			monitoringSystem.RecordSecurityEvent("monitoring_data_export_failure", "high", "monitor_command")
			monitoringSystem.CaptureSecurityError(err, map[string]interface{}{
				"command": "monitor export",
				"output":  export,
				"source":  "monitor_command",
			})
			
			return fmt.Errorf("failed to export monitoring data: %w", err)
		}

		// Write to file
		err = writeExportFile(export, exportData)
		if err != nil {
			return fmt.Errorf("failed to write export file: %w", err)
		}

		// Log export success
		successEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_data_export_success",
			Severity:  "low",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command":      "monitor export",
				"output":       export,
				"record_count": len(exportData.Records),
				"file_size":   exportData.FileSize,
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(successEvent)
		monitoringSystem.RecordSecurityEvent("monitoring_data_export_success", "low", "monitor_command")
		monitoringSystem.TrackSecurityEvent("monitoring_data_exported", map[string]interface{}{
			"output":       export,
			"record_count": len(exportData.Records),
			"file_size":   exportData.FileSize,
			"source":      "monitor_command",
		})

		fmt.Printf("✅ Successfully exported %d records\n", len(exportData.Records))
		fmt.Printf("📄 File size: %s\n", formatBytes(exportData.FileSize))
		fmt.Printf("📁 Exported to: %s\n", export)

		return nil
	},
}

var monitorAlertsCmd = &cobra.Command{
	Use:   "alerts",
	Short: "Show monitoring alerts",
	Long:  `Display active and historical monitoring alerts from the system.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		// Log alerts check event
		alertsEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_alerts_checked",
			Severity:  "info",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command": "monitor alerts",
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(alertsEvent)

		fmt.Println("🚨 Vaughan CLI Monitoring Alerts")

		// Get monitoring engine
		monitoringEngine := monitoring.NewMonitoringEngine(context.Background())

		// Get alerts
		alerts, err := monitoringEngine.GetAlerts()
		if err != nil {
			return fmt.Errorf("failed to get alerts: %w", err)
		}

		// Sort alerts by severity and time
		sort.Slice(alerts, func(i, j int) bool {
			if alerts[i].Severity != alerts[j].Severity {
				return getSeverityWeight(alerts[i].Severity) > getSeverityWeight(alerts[j].Severity)
			}
			return alerts[i].Timestamp.After(alerts[j].Timestamp)
		})

		if len(alerts) == 0 {
			fmt.Println("✅ No active alerts")
			return nil
		}

		fmt.Printf("\n📊 Found %d alerts:\n\n", len(alerts))
		for _, alert := range alerts {
			severityIcon := getSeverityIcon(alert.Severity)
			fmt.Printf("%s %s (%s)\n", severityIcon, alert.Title, alert.Severity)
			fmt.Printf("   📅 Time: %s\n", alert.Timestamp.Format(time.RFC3339))
			fmt.Printf("   📝 Description: %s\n", alert.Description)
			fmt.Printf("   🔍 Source: %s\n", alert.Source)
			if alert.Resolved {
				fmt.Printf("   ✅ Resolved: %s\n", alert.ResolvedAt.Format(time.RFC3339))
			}
			fmt.Println()
		}

		// Log alerts completion
		completionEvent := &real.SecurityEvent{
			ID:        generateMonitorEventID(),
			Type:      "monitoring_alerts_completed",
			Severity:  "info",
			Source:    "monitor_command",
			UserID:    getCurrentUserID(),
			IPAddress: getClientIP(),
			UserAgent: getUserAgent(),
			Details: map[string]interface{}{
				"command":     "monitor alerts",
				"alert_count": len(alerts),
			},
			Timestamp: time.Now(),
		}
		
		monitoringSystem.LogSecurityEvent(completionEvent)

		return nil
	},
}

// Helper functions
func generateMonitorEventID() string {
	return fmt.Sprintf("mon_%d", time.Now().UnixNano())
}

func getCurrentUserID() string {
	// In production, get current authenticated user ID
	return "system_user"
}

func getClientIP() string {
	// In production, get real client IP
	return "127.0.0.1"
}

func getUserAgent() string {
	// In production, get real user agent
	return "vaughan-cli/1.0.0"
}

func exportMetrics(filename string, currentMetrics *monitoring.CurrentMetrics, securityMetrics *real.MonitoringMetrics) error {
	file, err := os.Create(filename)
	if err != nil {
		return err
	}
	defer file.Close()

	// Write metrics as JSON
	exportData := map[string]interface{}{
		"timestamp": time.Now(),
		"security":  securityMetrics,
		"system":    currentMetrics,
	}

	data, err := json.Marshal(exportData)
	if err != nil {
		return err
	}

	_, err = file.Write(data)
	return err
}

func writeExportFile(filename string, exportData *monitoring.ExportData) error {
	file, err := os.Create(filename)
	if err != nil {
		return err
	}
	defer file.Close()

	return json.NewEncoder(file).Encode(exportData)
}

func formatBytes(bytes int64) string {
	const unit = 1024
	if bytes < unit {
		return fmt.Sprintf("%d B", bytes)
	}
	div, exp := int64(unit), 0
	for n := bytes / unit; n >= unit; n /= unit {
		div *= unit
		exp++
	}
	return fmt.Sprintf("%.1f %cB", float64(bytes)/float64(div), "KMGTPE"[exp])
}

func getSeverityWeight(severity string) int {
	switch severity {
	case "critical":
		return 5
	case "high":
		return 4
	case "medium":
		return 3
	case "low":
		return 2
	case "info":
		return 1
	default:
		return 0
	}
}

func getSeverityIcon(severity string) string {
	switch severity {
	case "critical":
		return "🔴"
	case "high":
		return "🟠"
	case "medium":
		return "🟡"
	case "low":
		return "🟢"
	case "info":
		return "🔵"
	default:
		return "⚪"
	}
}