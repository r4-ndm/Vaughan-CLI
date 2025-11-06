package plugin

import (
	"fmt"
	"reflect"
	"sync"

	"github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// Plugin represents a loadable plugin
type Plugin struct {
	Name        string
	Version     string
	Description string
	Author      string
	Implements  []string
	Instance    interface{}
}

// System manages plugin loading and lifecycle
type System struct {
	plugins map[string]*Plugin
	mutex   sync.RWMutex
}

// NewSystem creates a new plugin system
func NewSystem() *System {
	return &System{
		plugins: make(map[string]*Plugin),
	}
}

// RegisterPlugin registers a plugin
func (s *System) RegisterPlugin(plugin *Plugin) error {
	s.mutex.Lock()
	defer s.mutex.Unlock()

	if _, exists := s.plugins[plugin.Name]; exists {
		return fmt.Errorf("plugin already registered: %s", plugin.Name)
	}

	s.plugins[plugin.Name] = plugin
	return nil
}

// GetPlugin returns a plugin by name
func (s *System) GetPlugin(name string) (*Plugin, error) {
	s.mutex.RLock()
	defer s.mutex.RUnlock()

	plugin, exists := s.plugins[name]
	if !exists {
		return nil, fmt.Errorf("plugin not found: %s", name)
	}

	return plugin, nil
}

// GetPluginsByInterface returns plugins implementing specific interface
func (s *System) GetPluginsByInterface(interfaceName string) []*Plugin {
	s.mutex.RLock()
	defer s.mutex.RUnlock()

	var result []*Plugin
	for _, plugin := range s.plugins {
		for _, iface := range plugin.Implements {
			if iface == interfaceName {
				result = append(result, plugin)
				break
			}
		}
	}

	return result
}

// GetPluginAs returns plugin cast to specific interface
func (s *System) GetPluginAs(name string, target interface{}) error {
	plugin, err := s.GetPlugin(name)
	if err != nil {
		return err
	}

	targetValue := reflect.ValueOf(target).Elem()
	pluginValue := reflect.ValueOf(plugin.Instance)

	if !pluginValue.Type().Implements(targetValue.Type()) {
		return fmt.Errorf("plugin %s does not implement required interface", name)
	}

	targetValue.Set(pluginValue)
	return nil
}

// UnregisterPlugin removes a plugin
func (s *System) UnregisterPlugin(name string) error {
	s.mutex.Lock()
	defer s.mutex.Unlock()

	if _, exists := s.plugins[name]; !exists {
		return fmt.Errorf("plugin not found: %s", name)
	}

	delete(s.plugins, name)
	return nil
}

// ListPlugins returns all registered plugins
func (s *System) ListPlugins() []*Plugin {
	s.mutex.RLock()
	defer s.mutex.RUnlock()

	result := make([]*Plugin, 0, len(s.plugins))
	for _, plugin := range s.plugins {
		result = append(result, plugin)
	}

	return result
}

// GetPluginInfo returns plugin information
func (s *System) GetPluginInfo(name string) (*PluginInfo, error) {
	plugin, err := s.GetPlugin(name)
	if err != nil {
		return nil, err
	}

	return &PluginInfo{
		Name:        plugin.Name,
		Version:     plugin.Version,
		Description: plugin.Description,
		Author:      plugin.Author,
		Implements:  plugin.Implements,
	}, nil
}

// PluginInfo contains plugin metadata
type PluginInfo struct {
	Name        string
	Version     string
	Description string
	Author      string
	Implements  []string
}

// ValidatePlugin checks if plugin is valid
func ValidatePlugin(plugin *Plugin) error {
	if plugin.Name == "" {
		return fmt.Errorf("plugin name is required")
	}

	if plugin.Version == "" {
		return fmt.Errorf("plugin version is required")
	}

	if plugin.Instance == nil {
		return fmt.Errorf("plugin instance is required")
	}

	if len(plugin.Implements) == 0 {
		return fmt.Errorf("plugin must implement at least one interface")
	}

	return nil
}

// LoadPlugin validates and registers a plugin
func (s *System) LoadPlugin(plugin *Plugin) error {
	if err := ValidatePlugin(plugin); err != nil {
		return fmt.Errorf("invalid plugin: %w", err)
	}

	return s.RegisterPlugin(plugin)
}

// RegisterBlockchainPlugin registers a blockchain network plugin
func (s *System) RegisterBlockchainPlugin(name, version, description, author string, network interfaces.BlockchainNetwork) error {
	plugin := &Plugin{
		Name:        name,
		Version:     version,
		Description: description,
		Author:      author,
		Implements:  []string{"BlockchainNetwork"},
		Instance:    network,
	}

	return s.LoadPlugin(plugin)
}

// RegisterConfigPlugin registers a config manager plugin
func (s *System) RegisterConfigPlugin(name, version, description, author string, configManager interfaces.ConfigManager) error {
	plugin := &Plugin{
		Name:        name,
		Version:     version,
		Description: description,
		Author:      author,
		Implements:  []string{"ConfigManager"},
		Instance:    configManager,
	}

	return s.LoadPlugin(plugin)
}