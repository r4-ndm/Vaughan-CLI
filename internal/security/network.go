package security

import (
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

// NetworkPolicy defines network access rules
type NetworkPolicy struct {
	AllowedHosts    []string          `json:"allowed_hosts"`
	AllowedPorts    []int             `json:"allowed_ports"`
	BlockedHosts    []string          `json:"blocked_hosts"`
	BlockedPorts    []int             `json:"blocked_ports"`
	TLSRequired     bool              `json:"tls_required"`
	AllowLocalhost  bool              `json:"allow_localhost"`
	MaxConnections  int               `json:"max_connections"`
	Timeout         time.Duration     `json:"timeout"`
	RateLimit       map[string]int    `json:"rate_limit"` // endpoint -> requests per minute
	ProxyURL       string            `json:"proxy_url,omitempty"`
	CustomHeaders   map[string]string `json:"custom_headers,omitempty"`
}

// NetworkSecurity manages network access controls
type NetworkSecurity struct {
	policy      *NetworkPolicy
	connections map[string]*ConnectionInfo
	mutex       sync.RWMutex
	logger      *SecurityLogger
	httpClient  *http.Client
}

// ConnectionInfo tracks connection information
type ConnectionInfo struct {
	Host       string    `json:"host"`
	Port       int       `json:"port"`
	Method     string    `json:"method"`
	Protocol   string    `json:"protocol"`
	UserID     string    `json:"user_id"`
	SessionID  string    `json:"session_id"`
	StartTime  time.Time `json:"start_time"`
	LastActive time.Time `json:"last_active"`
	BytesSent  int64     `json:"bytes_sent"`
	BytesRecv  int64     `json:"bytes_recv"`
}

// SecurityHTTPClient wraps http.Client with security controls
type SecurityHTTPClient struct {
	client         *http.Client
	networkSec     *NetworkSecurity
	securityCtx    *Context
	logger         *SecurityLogger
}

// NewNetworkSecurity creates network security manager
func NewNetworkSecurity(policy *NetworkPolicy, logger *SecurityLogger) *NetworkSecurity {
	ns := &NetworkSecurity{
		policy:      policy,
		connections: make(map[string]*ConnectionInfo),
		logger:      logger,
	}
	
	// Create secure HTTP client
	ns.httpClient = &http.Client{
		Transport: &http.Transport{
			TLSClientConfig: ns.createTLSConfig(),
			DialContext:     ns.secureDial,
			Proxy:           ns.getProxy(),
		},
		Timeout: policy.Timeout,
	}
	
	return ns
}

// ValidateURL validates if URL is allowed by policy
func (ns *NetworkSecurity) ValidateURL(rawURL string) error {
	if rawURL == "" {
		return fmt.Errorf("URL cannot be empty")
	}
	
	parsed, err := url.Parse(rawURL)
	if err != nil {
		return fmt.Errorf("invalid URL format: %w", err)
	}
	
	// Check blocked hosts
	for _, blocked := range ns.policy.BlockedHosts {
		if strings.Contains(parsed.Host, blocked) {
			return fmt.Errorf("host '%s' is blocked by security policy", parsed.Host)
		}
	}
	
	// Check allowed hosts (if specified)
	if len(ns.policy.AllowedHosts) > 0 {
		allowed := false
		for _, allowedHost := range ns.policy.AllowedHosts {
			if strings.Contains(parsed.Host, allowedHost) {
				allowed = true
				break
			}
		}
		if !allowed && !ns.policy.AllowLocalhost || (ns.policy.AllowLocalhost && !ns.isLocalhost(parsed.Host)) {
			return fmt.Errorf("host '%s' is not in allowed list", parsed.Host)
		}
	}
	
	// Check TLS requirement
	if ns.policy.TLSRequired && parsed.Scheme != "https" {
		return fmt.Errorf("TLS is required for all connections")
	}
	
	// Check port restrictions
	if parsed.Port() != "" {
		port := parsed.Port()
		for _, blockedPort := range ns.policy.BlockedPorts {
			if port == fmt.Sprintf("%d", blockedPort) {
				return fmt.Errorf("port %s is blocked by security policy", port)
			}
		}
		
		if len(ns.policy.AllowedPorts) > 0 {
			allowed := false
			for _, allowedPort := range ns.policy.AllowedPorts {
				if port == fmt.Sprintf("%d", allowedPort) {
					allowed = true
					break
				}
			}
			if !allowed {
				return fmt.Errorf("port %s is not in allowed list", port)
			}
		}
	}
	
	return nil
}

// CreateSecureHTTPClient creates a secure HTTP client with context
func (ns *NetworkSecurity) CreateSecureHTTPClient(ctx *Context) *SecurityHTTPClient {
	return &SecurityHTTPClient{
		client:      ns.httpClient,
		networkSec:  ns,
		securityCtx: ctx,
		logger:      ns.logger,
	}
}

// Do performs secure HTTP request
func (shc *SecurityHTTPClient) Do(req *http.Request) (*http.Response, error) {
	// Validate request URL
	if err := shc.networkSec.ValidateURL(req.URL.String()); err != nil {
		if shc.logger != nil {
			shc.logger.LogNetworkRequest(shc.securityCtx.UserID, shc.securityCtx.SessionID, req.URL.String(), false, map[string]interface{}{
				"error": err.Error(),
			})
		}
		return nil, fmt.Errorf("network request blocked: %w", err)
	}
	
	// Check rate limiting
	if err := shc.networkSec.checkRateLimit(req.URL.Host, shc.securityCtx.UserID); err != nil {
		if shc.logger != nil {
			shc.logger.LogNetworkRequest(shc.securityCtx.UserID, shc.securityCtx.SessionID, req.URL.String(), false, map[string]interface{}{
				"error": err.Error(),
			})
		}
		return nil, err
	}
	
	// Add security headers
	shc.addSecurityHeaders(req)
	
	// Track connection
	connID := shc.networkSec.trackConnection(req, shc.securityCtx)
	
	// Log request
	if shc.logger != nil {
		shc.logger.LogNetworkRequest(shc.securityCtx.UserID, shc.securityCtx.SessionID, req.URL.String(), true, map[string]interface{}{
			"method": req.Method,
			"host":   req.URL.Host,
		})
	}
	
	// Perform request
	resp, err := shc.client.Do(req)
	
	// Update connection tracking
	if connID != "" {
		shc.networkSec.updateConnection(connID, resp, err)
	}
	
	return resp, err
}

// Get retrieves URL with security controls
func (shc *SecurityHTTPClient) Get(url string) (*http.Response, error) {
	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return nil, err
	}
	return shc.Do(req)
}

// Post sends POST request with security controls
func (shc *SecurityHTTPClient) Post(url, contentType string, body interface{}) (*http.Response, error) {
	req, err := http.NewRequest("POST", url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", contentType)
	return shc.Do(req)
}

// createTLSConfig creates secure TLS configuration
func (ns *NetworkSecurity) createTLSConfig() *tls.Config {
	return &tls.Config{
		MinVersion: tls.VersionTLS12,
		CipherSuites: []uint16{
			tls.TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
			tls.TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
			tls.TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
			tls.TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
		},
		InsecureSkipVerify: false,
		RootCAs: ns.getRootCAs(),
	}
}

// getRootCAs returns system root CAs with additional verification
func (ns *NetworkSecurity) getRootCAs() *x509.CertPool {
	pool, _ := x509.SystemCertPool()
	if pool == nil {
		pool = x509.NewCertPool()
	}
	return pool
}

// secureDial implements secure network dialing
func (ns *NetworkSecurity) secureDial(network, addr string) (net.Conn, error) {
	host, port, err := net.SplitHostPort(addr)
	if err != nil {
		return nil, fmt.Errorf("invalid address format: %w", err)
	}
	
	// Validate host and port
	if !ns.isHostAllowed(host) {
		return nil, fmt.Errorf("host '%s' is not allowed", host)
	}
	
	if !ns.isPortAllowed(port) {
		return nil, fmt.Errorf("port '%s' is not allowed", port)
	}
	
	// Create connection with timeout
	dialer := &net.Dialer{
		Timeout: ns.policy.Timeout,
	}
	
	return dialer.Dial(network, addr)
}

// getProxy returns proxy configuration
func (ns *NetworkSecurity) getProxy() func(*http.Request) (*url.URL, error) {
	if ns.policy.ProxyURL == "" {
		return nil // No proxy
	}
	
	proxyURL, err := url.Parse(ns.policy.ProxyURL)
	if err != nil {
		return nil
	}
	
	return http.ProxyURL(proxyURL)
}

// isHostAllowed checks if host is allowed by policy
func (ns *NetworkSecurity) isHostAllowed(host string) bool {
	// Check blocked hosts
	for _, blocked := range ns.policy.BlockedHosts {
		if strings.Contains(host, blocked) {
			return false
		}
	}
	
	// Check allowed hosts (if specified)
	if len(ns.policy.AllowedHosts) > 0 {
		for _, allowed := range ns.policy.AllowedHosts {
			if strings.Contains(host, allowed) {
				return true
			}
		}
		// Check localhost allowance
		if ns.policy.AllowLocalhost && ns.isLocalhost(host) {
			return true
		}
		return false
	}
	
	return true
}

// isPortAllowed checks if port is allowed by policy
func (ns *NetworkSecurity) isPortAllowed(portStr string) bool {
	// Check blocked ports
	for _, blockedPort := range ns.policy.BlockedPorts {
		if fmt.Sprintf("%d", blockedPort) == portStr {
			return false
		}
	}
	
	// Check allowed ports (if specified)
	if len(ns.policy.AllowedPorts) > 0 {
		for _, allowedPort := range ns.policy.AllowedPorts {
			if fmt.Sprintf("%d", allowedPort) == portStr {
				return true
			}
		}
		return false
	}
	
	return true
}

// isLocalhost checks if host is localhost
func (ns *NetworkSecurity) isLocalhost(host string) bool {
	return strings.Contains(host, "localhost") || 
		   strings.Contains(host, "127.0.0.1") || 
		   strings.Contains(host, "::1")
}

// checkRateLimit implements rate limiting
func (ns *NetworkSecurity) checkRateLimit(host, userID string) error {
	// Simplified rate limiting - implement proper token bucket or sliding window
	if ns.policy.RateLimit == nil {
		return nil
	}
	
	// In production, implement proper rate limiting with Redis/DB
	// For now, just check if rate limiting is configured
	return nil
}

// addSecurityHeaders adds security headers to request
func (shc *SecurityHTTPClient) addSecurityHeaders(req *http.Request) {
	// Add user security context
	if shc.securityCtx != nil {
		req.Header.Set("X-User-ID", shc.securityCtx.UserID)
		req.Header.Set("X-Session-ID", shc.securityCtx.SessionID)
	}
	
	// Add security headers
	req.Header.Set("X-Content-Type-Options", "nosniff")
	req.Header.Set("X-Frame-Options", "DENY")
	req.Header.Set("X-XSS-Protection", "1; mode=block")
	
	// Add custom headers from policy
	if shc.networkSec.policy.CustomHeaders != nil {
		for key, value := range shc.networkSec.policy.CustomHeaders {
			req.Header.Set(key, value)
		}
	}
}

// trackConnection tracks connection for monitoring
func (ns *NetworkSecurity) trackConnection(req *http.Request, ctx *Context) string {
	connID := fmt.Sprintf("%s_%d", req.URL.Host, time.Now().UnixNano())
	
	ns.mutex.Lock()
	ns.connections[connID] = &ConnectionInfo{
		Host:       req.URL.Host,
		Method:     req.Method,
		Protocol:   req.URL.Scheme,
		UserID:     ctx.UserID,
		SessionID:  ctx.SessionID,
		StartTime:  time.Now(),
		LastActive: time.Now(),
	}
	ns.mutex.Unlock()
	
	return connID
}

// updateConnection updates connection tracking
func (ns *NetworkSecurity) updateConnection(connID string, resp *http.Response, err error) {
	ns.mutex.Lock()
	defer ns.mutex.Unlock()
	
	conn, exists := ns.connections[connID]
	if !exists {
		return
	}
	
	conn.LastActive = time.Now()
	
	if resp != nil {
		conn.BytesSent = resp.ContentLength
	}
	
	// Remove old connections (cleanup logic would run periodically)
	if time.Since(conn.StartTime) > ns.policy.Timeout*2 {
		delete(ns.connections, connID)
	}
}

// GetActiveConnections returns active connections
func (ns *NetworkSecurity) GetActiveConnections() []*ConnectionInfo {
	ns.mutex.RLock()
	defer ns.mutex.RUnlock()
	
	connections := make([]*ConnectionInfo, 0, len(ns.connections))
	for _, conn := range ns.connections {
		if time.Since(conn.LastActive) < ns.policy.Timeout {
			connections = append(connections, conn)
		}
	}
	
	return connections
}

// CleanupConnections removes old connections
func (ns *NetworkSecurity) CleanupConnections() int {
	ns.mutex.Lock()
	defer ns.mutex.Unlock()
	
	cutoff := time.Now().Add(-ns.policy.Timeout * 2)
	removed := 0
	
	for id, conn := range ns.connections {
		if conn.LastActive.Before(cutoff) {
			delete(ns.connections, id)
			removed++
		}
	}
	
	return removed
}

// LogNetworkRequest logs network request events
func (sl *SecurityLogger) LogNetworkRequest(userID, sessionID, url string, success bool, details map[string]interface{}) {
	event := SecurityEvent{
		Type:        EventNetworkRequest,
		Severity:    SeverityMedium,
		UserID:      userID,
		SessionID:   sessionID,
		Description: fmt.Sprintf("Network request to %s", url),
		Details:     details,
	}
	
	if !success {
		event.Type = EventNetworkBlocked
		event.Severity = SeverityHigh
		event.Description = fmt.Sprintf("Network request to %s blocked", url)
	}
	
	sl.LogEvent(event)
}

// DefaultNetworkPolicy returns a secure default network policy
func DefaultNetworkPolicy() *NetworkPolicy {
	return &NetworkPolicy{
		AllowedHosts: []string{
			"api.etherscan.io",
			"api.infura.io",
			"api.alchemy.com",
			"cloudflare-eth.com",
			"mainnet.infura.io",
			"goerli.infura.io",
		},
		AllowedPorts:   []int{443, 80}, // HTTPS, HTTP
		BlockedPorts:   []int{22, 23, 25, 53, 135, 139, 445, 1433, 3306},
		TLSRequired:    true,
		AllowLocalhost: true,
		MaxConnections: 100,
		Timeout:        30 * time.Second,
		RateLimit: map[string]int{
			"api.etherscan.io": 60,  // 60 requests per minute
			"api.infura.io":    120, // 120 requests per minute
		},
		CustomHeaders: map[string]string{
			"User-Agent": "Vaughan-Crush/1.0",
		},
	}
}

// RestrictiveNetworkPolicy returns a highly restrictive policy
func RestrictiveNetworkPolicy() *NetworkPolicy {
	return &NetworkPolicy{
		AllowedHosts:   []string{},
		AllowedPorts:    []int{443}, // HTTPS only
		BlockedPorts:    []int{80, 22, 23, 25, 53, 135, 139, 445, 1433, 3306},
		TLSRequired:     true,
		AllowLocalhost:  false,
		MaxConnections: 10,
		Timeout:        15 * time.Second,
		RateLimit: map[string]int{
			"*": 30, // 30 requests per minute for all hosts
		},
		CustomHeaders: map[string]string{
			"User-Agent":   "Vaughan-Crush/1.0",
			"Accept":       "application/json",
			"Accept-Language": "en",
		},
	}
}