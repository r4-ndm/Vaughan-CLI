package events

import (
	"fmt"
	"reflect"
	"sync"
	"time"

	"github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// Event represents a system event
type Event struct {
	Type      string
	Data      interface{}
	Timestamp time.Time
	Source    string
	ID        string
}

// Handler represents an event handler
type Handler func(Event) error

// Filter represents an event filter
type Filter func(Event) bool

// System manages event publishing and handling
type System struct {
	handlers map[string][]Handler
	filters  map[string][]Filter
	mutex    sync.RWMutex
}

// NewSystem creates a new event system
func NewSystem() *System {
	return &System{
		handlers: make(map[string][]Handler),
		filters:  make(map[string][]Filter),
	}
}

// Subscribe subscribes to events of a specific type
func (s *System) Subscribe(eventType string, handler Handler) {
	s.mutex.Lock()
	defer s.mutex.Unlock()

	s.handlers[eventType] = append(s.handlers[eventType], handler)
}

// SubscribeWithFilter subscribes to events with a filter
func (s *System) SubscribeWithFilter(eventType string, handler Handler, filter Filter) {
	s.mutex.Lock()
	defer s.mutex.Unlock()

	s.handlers[eventType] = append(s.handlers[eventType], handler)
	s.filters[eventType] = append(s.filters[eventType], filter)
}

// Publish publishes an event to all subscribers
func (s *System) Publish(event Event) error {
	if event.ID == "" {
		event.ID = fmt.Sprintf("%d-%d", time.Now().UnixNano(), len(event.Type))
	}

	if event.Timestamp.IsZero() {
		event.Timestamp = time.Now()
	}

	s.mutex.RLock()
	handlers := s.handlers[event.Type]
	filters := s.filters[event.Type]
	s.mutex.RUnlock()

	for i, handler := range handlers {
		// Apply filter if available
		if i < len(filters) && filters[i] != nil {
			if !filters[i](event) {
				continue
			}
		}

		if err := handler(event); err != nil {
			return fmt.Errorf("event handler failed: %w", err)
		}
	}

	return nil
}

// PublishAsync publishes an event asynchronously
func (s *System) PublishAsync(event Event) {
	go func() {
		if err := s.Publish(event); err != nil {
			// Log error - in real implementation, use proper logging
			fmt.Printf("Event publish failed: %v\n", err)
		}
	}()
}

// Unsubscribe removes a handler
func (s *System) Unsubscribe(eventType string, handler Handler) {
	s.mutex.Lock()
	defer s.mutex.Unlock()

	handlers := s.handlers[eventType]
	for i, h := range handlers {
		if reflect.ValueOf(h).Pointer() == reflect.ValueOf(handler).Pointer() {
			s.handlers[eventType] = append(handlers[:i], handlers[i+1:]...)
			break
		}
	}

	// Remove corresponding filter
	filters := s.filters[eventType]
	if i < len(filters) {
		s.filters[eventType] = append(filters[:i], filters[i+1:]...)
	}
}

// Clear removes all handlers for an event type
func (s *System) Clear(eventType string) {
	s.mutex.Lock()
	defer s.mutex.Unlock()

	delete(s.handlers, eventType)
	delete(s.filters, eventType)
}

// ClearAll removes all handlers
func (s *System) ClearAll() {
	s.mutex.Lock()
	defer s.mutex.Unlock()

	s.handlers = make(map[string][]Handler)
	s.filters = make(map[string][]Filter)
}

// GetHandlerCount returns the number of handlers for an event type
func (s *System) GetHandlerCount(eventType string) int {
	s.mutex.RLock()
	defer s.mutex.RUnlock()

	return len(s.handlers[eventType])
}

// GetSubscribedEvents returns all subscribed event types
func (s *System) GetSubscribedEvents() []string {
	s.mutex.RLock()
	defer s.mutex.RUnlock()

	events := make([]string, 0, len(s.handlers))
	for eventType := range s.handlers {
		events = append(events, eventType)
	}

	return events
}

// PublishBatch publishes multiple events
func (s *System) PublishBatch(events []Event) error {
	for _, event := range events {
		if err := s.Publish(event); err != nil {
			return fmt.Errorf("failed to publish event %s: %w", event.ID, err)
		}
	}

	return nil
}

// PublishError publishes an error event
func (s *System) PublishError(err error, source string) {
	event := Event{
		Type:   "error",
		Data:   err,
		Source: source,
	}

	s.PublishAsync(event)
}

// PublishNetworkChange publishes a network change event
func (s *System) PublishNetworkChange(oldNetwork, newNetwork interfaces.BlockchainNetwork, source string) {
	event := Event{
		Type:   "network_change",
		Data: map[string]interface{}{
			"old_network": oldNetwork,
			"new_network": newNetwork,
		},
		Source: source,
	}

	s.PublishAsync(event)
}

// PublishConfigChange publishes a configuration change event
func (s *System) PublishConfigChange(changes map[string]interface{}, source string) {
	event := Event{
		Type:   "config_change",
		Data:   changes,
		Source: source,
	}

	s.PublishAsync(event)
}

// PublishTransaction publishes a transaction event
func (s *System) PublishTransaction(txHash string, status string, data interface{}, source string) {
	event := Event{
		Type:   "transaction",
		Data: map[string]interface{}{
			"hash":   txHash,
			"status": status,
			"data":   data,
		},
		Source: source,
	}

	s.PublishAsync(event)
}

// PublishPluginEvent publishes a plugin-related event
func (s *System) PublishPluginEvent(pluginName, action string, data interface{}, source string) {
	event := Event{
		Type:   "plugin",
		Data: map[string]interface{}{
			"plugin": pluginName,
			"action": action,
			"data":   data,
		},
		Source: source,
	}

	s.PublishAsync(event)
}

// CreateFilter creates a simple filter function
func CreateFilter(eventType string, source string) Filter {
	return func(event Event) bool {
		return event.Type == eventType && event.Source == source
	}
}

// CreateDataFilter creates a filter based on event data
func CreateDataFilter(eventType string, key string, value interface{}) Filter {
	return func(event Event) bool {
		if event.Type != eventType {
			return false
		}

		if data, ok := event.Data.(map[string]interface{}); ok {
			if eventValue, exists := data[key]; exists {
				return reflect.DeepEqual(eventValue, value)
			}
		}

		return false
	}
}

// CreateSourceFilter creates a filter based on event source
func CreateSourceFilter(source string) Filter {
	return func(event Event) bool {
		return event.Source == source
	}
}