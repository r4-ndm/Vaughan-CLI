package event

import (
	"time"
)

var appStartTime time.Time

func AppInitialized() {
	appStartTime = time.Now()
	// Analytics disabled for privacy
}

func AppExited() {
	duration := time.Since(appStartTime).Truncate(time.Second)
	// Analytics disabled for privacy
	_ = duration
	Flush()
}

func SessionCreated() {
	// Analytics disabled for privacy
}

func SessionDeleted() {
	// Analytics disabled for privacy
}

func SessionSwitched() {
	// Analytics disabled for privacy
}

func FilePickerOpened() {
	// Analytics disabled for privacy
}

func PromptSent(props ...any) {
	// Analytics disabled for privacy
	_ = props
}

func PromptResponded(props ...any) {
	// Analytics disabled for privacy
	_ = props
}

func TokensUsed(props ...any) {
	// Analytics disabled for privacy
	_ = props
}
