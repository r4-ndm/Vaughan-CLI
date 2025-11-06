package event

// All event functions are no-ops for privacy
// No analytics or telemetry data is collected or sent

import "log/slog"

func Init() {
	slog.Debug("Event system disabled for privacy")
}

func Error(err any, props ...any) {
	// Silent error handling - no external logging
}

func Flush() {
	// No-op - nothing to flush
}