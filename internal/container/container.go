package container

import (
	"fmt"
	"reflect"
	"sync"

	"github.com/r4v3n/vaughan-cli/internal/interfaces"
)

// Container manages dependency injection
type Container struct {
	services map[string]interface{}
	factories map[string]func() interface{}
	singletons map[string]interface{}
	mutex    sync.RWMutex
}

// NewContainer creates a new dependency container
func NewContainer() *Container {
	return &Container{
		services:   make(map[string]interface{}),
		factories:   make(map[string]func() interface{}),
		singletons: make(map[string]interface{}),
	}
}

// Register registers a service with the container
func (c *Container) Register(name string, service interface{}) {
	c.mutex.Lock()
	defer c.mutex.Unlock()

	c.services[name] = service
}

// RegisterFactory registers a factory function for lazy loading
func (c *Container) RegisterFactory(name string, factory func() interface{}) {
	c.mutex.Lock()
	defer c.mutex.Unlock()

	c.factories[name] = factory
}

// RegisterSingleton registers a singleton service
func (c *Container) RegisterSingleton(name string, service interface{}) {
	c.mutex.Lock()
	defer c.mutex.Unlock()

	c.singletons[name] = service
}

// Resolve resolves a service by name
func (c *Container) Resolve(name string) (interface{}, error) {
	c.mutex.RLock()
	defer c.mutex.RUnlock()

	// Check singletons first
	if singleton, exists := c.singletons[name]; exists {
		return singleton, nil
	}

	// Check regular services
	if service, exists := c.services[name]; exists {
		return service, nil
	}

	// Check factories
	if factory, exists := c.factories[name]; exists {
		return factory(), nil
	}

	return nil, fmt.Errorf("service not found: %s", name)
}

// ResolveInterface resolves a service by interface type
func (c *Container) ResolveInterface(target interface{}) error {
	c.mutex.RLock()
	defer c.mutex.RUnlock()

	targetType := reflect.TypeOf(target).Elem()
	targetName := targetType.String()

	// Try to find service by type name
	if service, exists := c.services[targetName]; exists {
		serviceType := reflect.TypeOf(service)
		if serviceType.Implements(targetType) {
			reflect.ValueOf(target).Elem().Set(reflect.ValueOf(service))
			return nil
		}
	}

	// Try factories
	if factory, exists := c.factories[targetName]; exists {
		service := factory()
		serviceType := reflect.TypeOf(service)
		if serviceType.Implements(targetType) {
			reflect.ValueOf(target).Elem().Set(reflect.ValueOf(service))
			return nil
		}
	}

	// Try singletons
	if singleton, exists := c.singletons[targetName]; exists {
		singletonType := reflect.TypeOf(singleton)
		if singletonType.Implements(targetType) {
			reflect.ValueOf(target).Elem().Set(reflect.ValueOf(singleton))
			return nil
		}
	}

	return fmt.Errorf("no service found for interface: %s", targetName)
}

// RegisterBlockchainNetwork registers a blockchain network
func (c *Container) RegisterBlockchainNetwork(name string, network interfaces.BlockchainNetwork) {
	c.Register(name, network)
	c.RegisterFactory(fmt.Sprintf("BlockchainNetwork:%s", name), func() interface{} {
		return network
	})
}

// RegisterConfigManager registers a config manager
func (c *Container) RegisterConfigManager(name string, configManager interfaces.ConfigManager) {
	c.Register(name, configManager)
	c.RegisterSingleton("ConfigManager", configManager)
}

// ResolveBlockchainNetwork resolves a blockchain network
func (c *Container) ResolveBlockchainNetwork(name string) (interfaces.BlockchainNetwork, error) {
	service, err := c.Resolve(fmt.Sprintf("BlockchainNetwork:%s", name))
	if err != nil {
		return nil, err
	}

	network, ok := service.(interfaces.BlockchainNetwork)
	if !ok {
		return nil, fmt.Errorf("service is not a BlockchainNetwork: %s", name)
	}

	return network, nil
}

// ResolveConfigManager resolves a config manager
func (c *Container) ResolveConfigManager() (interfaces.ConfigManager, error) {
	service, err := c.Resolve("ConfigManager")
	if err != nil {
		return nil, err
	}

	configManager, ok := service.(interfaces.ConfigManager)
	if !ok {
		return nil, fmt.Errorf("service is not a ConfigManager")
	}

	return configManager, nil
}

// GetServiceNames returns all registered service names
func (c *Container) GetServiceNames() []string {
	c.mutex.RLock()
	defer c.mutex.RUnlock()

	names := make([]string, 0, len(c.services)+len(c.factories)+len(c.singletons))

	for name := range c.services {
		names = append(names, name)
	}

	for name := range c.factories {
		names = append(names, name)
	}

	for name := range c.singletons {
		names = append(names, name)
	}

	return names
}

// Clear removes all services from container
func (c *Container) Clear() {
	c.mutex.Lock()
	defer c.mutex.Unlock()

	c.services = make(map[string]interface{})
	c.factories = make(map[string]func() interface{})
	c.singletons = make(map[string]interface{})
}

// AutoRegister automatically registers all interface implementations
func (c *Container) AutoRegister() {
	// This can be enhanced with reflection to auto-discover implementations
	// For now, it's a placeholder for future enhancement
}

// HasService checks if a service is registered
func (c *Container) HasService(name string) bool {
	c.mutex.RLock()
	defer c.mutex.RUnlock()

	_, exists := c.services[name]
	if exists {
		return true
	}

	_, exists = c.factories[name]
	if exists {
		return true
	}

	_, exists = c.singletons[name]
	return exists
}

// GetServiceInfo returns information about a service
func (c *Container) GetServiceInfo(name string) *ServiceInfo {
	c.mutex.RLock()
	defer c.mutex.RUnlock()

	info := &ServiceInfo{Name: name}

	if service, exists := c.services[name]; exists {
		info.Value = service
		info.Type = "service"
		return info
	}

	if factory, exists := c.factories[name]; exists {
		info.Value = factory
		info.Type = "factory"
		return info
	}

	if singleton, exists := c.singletons[name]; exists {
		info.Value = singleton
		info.Type = "singleton"
		return info
	}

	return nil
}

// ServiceInfo contains information about a registered service
type ServiceInfo struct {
	Name  string
	Type  string // "service", "factory", "singleton"
	Value interface{}
}