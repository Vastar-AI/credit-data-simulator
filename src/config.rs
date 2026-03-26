//! # Simulator Configuration
//!
//! This module provides configuration structures for all simulators.
//! Each simulator has its own configuration with sensible defaults.

use chrono::Timelike;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

// ============================================================================
// Main Configuration
// ============================================================================

/// Configuration for all simulators
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatorConfig {
    /// Core Banking simulator configuration
    pub core_banking: CoreBankingConfig,
    /// Mapping Service simulator configuration
    pub mapping_service: MappingServiceConfig,
    /// Rulepack Service simulator configuration
    pub rulepack_service: RulepackServiceConfig,
    /// Regulator Endpoint simulator configuration
    pub regulator_endpoint: RegulatorEndpointConfig,
}

impl Default for SimulatorConfig {
    fn default() -> Self {
        Self {
            core_banking: CoreBankingConfig::default(),
            mapping_service: MappingServiceConfig::default(),
            rulepack_service: RulepackServiceConfig::default(),
            regulator_endpoint: RegulatorEndpointConfig::default(),
        }
    }
}

impl SimulatorConfig {
    /// Create a new configuration builder
    pub fn builder() -> SimulatorConfigBuilder {
        SimulatorConfigBuilder::default()
    }

    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(port) = std::env::var("CORE_BANKING_PORT") {
            if let Ok(p) = port.parse() {
                config.core_banking.port = p;
            }
        }
        if let Ok(port) = std::env::var("MAPPING_SERVICE_PORT") {
            if let Ok(p) = port.parse() {
                config.mapping_service.port = p;
            }
        }
        if let Ok(port) = std::env::var("RULEPACK_SERVICE_PORT") {
            if let Ok(p) = port.parse() {
                config.rulepack_service.port = p;
            }
        }
        if let Ok(port) = std::env::var("REGULATOR_ENDPOINT_PORT") {
            if let Ok(p) = port.parse() {
                config.regulator_endpoint.port = p;
            }
        }

        config
    }

    /// Create configuration for CI environment (faster timeouts, no delays)
    pub fn for_ci() -> Self {
        Self {
            core_banking: CoreBankingConfig {
                latency: LatencyConfig::none(),
                ..Default::default()
            },
            mapping_service: MappingServiceConfig {
                latency: LatencyConfig::none(),
                ..Default::default()
            },
            rulepack_service: RulepackServiceConfig {
                latency: LatencyConfig::none(),
                ..Default::default()
            },
            regulator_endpoint: RegulatorEndpointConfig {
                latency: LatencyConfig::none(),
                ..Default::default()
            },
        }
    }

    /// Create configuration for load testing (realistic latencies)
    pub fn for_load_test() -> Self {
        Self {
            core_banking: CoreBankingConfig {
                latency: LatencyConfig::realistic(),
                ..Default::default()
            },
            mapping_service: MappingServiceConfig {
                latency: LatencyConfig::realistic(),
                ..Default::default()
            },
            rulepack_service: RulepackServiceConfig {
                latency: LatencyConfig::realistic(),
                ..Default::default()
            },
            regulator_endpoint: RegulatorEndpointConfig {
                latency: LatencyConfig::realistic(),
                ..Default::default()
            },
        }
    }
}

/// Builder for SimulatorConfig
#[derive(Debug, Default)]
pub struct SimulatorConfigBuilder {
    config: SimulatorConfig,
}

impl SimulatorConfigBuilder {
    pub fn core_banking(mut self, config: CoreBankingConfig) -> Self {
        self.config.core_banking = config;
        self
    }

    pub fn mapping_service(mut self, config: MappingServiceConfig) -> Self {
        self.config.mapping_service = config;
        self
    }

    pub fn rulepack_service(mut self, config: RulepackServiceConfig) -> Self {
        self.config.rulepack_service = config;
        self
    }

    pub fn regulator_endpoint(mut self, config: RegulatorEndpointConfig) -> Self {
        self.config.regulator_endpoint = config;
        self
    }

    pub fn build(self) -> SimulatorConfig {
        self.config
    }
}

// ============================================================================
// Core Banking Configuration
// ============================================================================

/// Configuration for Core Banking Simulator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreBankingConfig {
    /// Whether this simulator is enabled
    pub enabled: bool,
    /// Port to listen on
    pub port: u16,
    /// Host to bind to
    pub host: String,
    /// Default number of records to generate
    pub default_record_count: u32,
    /// Maximum records per request
    pub max_records_per_request: u32,
    /// Dirty data ratio (0.0 - 1.0)
    pub default_dirty_ratio: f64,
    /// Random seed for deterministic generation (None = random)
    pub seed: Option<u64>,
    /// Latency configuration
    pub latency: LatencyConfig,
    /// Failure injection configuration
    pub failure_injection: FailureInjectionConfig,
    /// Data generation configuration
    pub data_generation: DataGenerationConfig,
}

impl Default for CoreBankingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 18081,
            host: "127.0.0.1".to_string(),
            default_record_count: 100_000,
            max_records_per_request: 50_000_000,
            default_dirty_ratio: 0.0,
            seed: None,
            latency: LatencyConfig::default(),
            failure_injection: FailureInjectionConfig::default(),
            data_generation: DataGenerationConfig::default(),
        }
    }
}

impl CoreBankingConfig {
    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn with_dirty_ratio(mut self, ratio: f64) -> Self {
        self.default_dirty_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    pub fn with_latency(mut self, latency: LatencyConfig) -> Self {
        self.latency = latency;
        self
    }
}

/// Configuration for data generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataGenerationConfig {
    /// Error types to inject and their weights
    pub error_types: HashMap<String, f64>,
    /// Whether to generate realistic-looking data
    pub realistic_data: bool,
    /// Date range for generated data (days back from now)
    pub date_range_days: u32,
    /// Currency to use
    pub currency: String,
    /// Minimum credit amount
    pub min_credit_amount: f64,
    /// Maximum credit amount
    pub max_credit_amount: f64,
}

impl Default for DataGenerationConfig {
    fn default() -> Self {
        let mut error_types = HashMap::new();
        error_types.insert("invalid_nik".to_string(), 0.3);
        error_types.insert("negative_amount".to_string(), 0.2);
        error_types.insert("invalid_date".to_string(), 0.2);
        error_types.insert("missing_field".to_string(), 0.15);
        error_types.insert("invalid_currency".to_string(), 0.1);
        error_types.insert("duplicate_record".to_string(), 0.05);

        Self {
            error_types,
            realistic_data: true,
            date_range_days: 365,
            currency: "IDR".to_string(),
            min_credit_amount: 1_000_000.0,
            max_credit_amount: 10_000_000_000.0,
        }
    }
}

// ============================================================================
// Mapping Service Configuration
// ============================================================================

/// Configuration for Mapping Service Simulator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingServiceConfig {
    /// Whether this simulator is enabled
    pub enabled: bool,
    /// Port to listen on
    pub port: u16,
    /// Host to bind to
    pub host: String,
    /// Available mapping versions
    pub available_versions: Vec<String>,
    /// Default version to return
    pub default_version: String,
    /// Latency configuration
    pub latency: LatencyConfig,
    /// Failure injection configuration
    pub failure_injection: FailureInjectionConfig,
    /// Whether to cache mappings in memory
    pub cache_enabled: bool,
}

impl Default for MappingServiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 18082,
            host: "127.0.0.1".to_string(),
            available_versions: vec!["v1".to_string(), "v2".to_string(), "v3".to_string()],
            default_version: "v2".to_string(),
            latency: LatencyConfig::default(),
            failure_injection: FailureInjectionConfig::default(),
            cache_enabled: true,
        }
    }
}

impl MappingServiceConfig {
    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn with_versions(mut self, versions: Vec<String>) -> Self {
        self.available_versions = versions;
        self
    }

    pub fn with_default_version(mut self, version: &str) -> Self {
        self.default_version = version.to_string();
        self
    }
}

// ============================================================================
// Rulepack Service Configuration
// ============================================================================

/// Configuration for Rulepack Service Simulator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulepackServiceConfig {
    /// Whether this simulator is enabled
    pub enabled: bool,
    /// Port to listen on
    pub port: u16,
    /// Host to bind to
    pub host: String,
    /// Available rulepack versions
    pub available_versions: Vec<String>,
    /// Default version to return
    pub default_version: String,
    /// Latency configuration
    pub latency: LatencyConfig,
    /// Failure injection configuration
    pub failure_injection: FailureInjectionConfig,
    /// Whether to include cross-field rules
    pub include_cross_field_rules: bool,
}

impl Default for RulepackServiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 18083,
            host: "127.0.0.1".to_string(),
            available_versions: vec!["v1".to_string(), "v2".to_string()],
            default_version: "v1".to_string(),
            latency: LatencyConfig::default(),
            failure_injection: FailureInjectionConfig::default(),
            include_cross_field_rules: true,
        }
    }
}

impl RulepackServiceConfig {
    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn with_versions(mut self, versions: Vec<String>) -> Self {
        self.available_versions = versions;
        self
    }
}

// ============================================================================
// Regulator Endpoint Configuration
// ============================================================================

/// Configuration for Regulator Endpoint Simulator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegulatorEndpointConfig {
    /// Whether this simulator is enabled
    pub enabled: bool,
    /// Port to listen on
    pub port: u16,
    /// Host to bind to
    pub host: String,
    /// Response mode
    pub mode: RegulatorMode,
    /// Latency configuration
    pub latency: LatencyConfig,
    /// Failure injection configuration
    pub failure_injection: FailureInjectionConfig,
    /// Whether to enforce idempotency
    pub enforce_idempotency: bool,
    /// Maximum submissions to track for idempotency
    pub max_idempotency_entries: usize,
    /// Idempotency entry TTL in seconds
    pub idempotency_ttl_secs: u64,
    /// Retry-After header value in seconds (for rate limiting)
    pub retry_after_secs: u32,
    /// Off-peak hours configuration
    pub off_peak_config: OffPeakConfig,
}

impl Default for RegulatorEndpointConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 18084,
            host: "127.0.0.1".to_string(),
            mode: RegulatorMode::Accept,
            latency: LatencyConfig::default(),
            failure_injection: FailureInjectionConfig::default(),
            enforce_idempotency: true,
            max_idempotency_entries: 10_000,
            idempotency_ttl_secs: 86400, // 24 hours
            retry_after_secs: 60,
            off_peak_config: OffPeakConfig::default(),
        }
    }
}

impl RegulatorEndpointConfig {
    pub fn socket_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn with_mode(mut self, mode: RegulatorMode) -> Self {
        self.mode = mode;
        self
    }

    /// Configure for timeout simulation
    pub fn timeout_mode(mut self, timeout_ms: u64) -> Self {
        self.mode = RegulatorMode::Timeout {
            delay_ms: timeout_ms,
        };
        self
    }

    /// Configure for rejection simulation
    pub fn reject_mode(mut self, code: &str, message: &str) -> Self {
        self.mode = RegulatorMode::Reject {
            error_code: code.to_string(),
            error_message: message.to_string(),
        };
        self
    }

    /// Configure for intermittent failures
    pub fn intermittent_mode(mut self, failure_rate: f64) -> Self {
        self.mode = RegulatorMode::Intermittent {
            failure_rate: failure_rate.clamp(0.0, 1.0),
        };
        self
    }
}

/// Regulator response modes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RegulatorMode {
    /// Accept all submissions
    Accept,
    /// Reject all submissions with specified error
    Reject {
        error_code: String,
        error_message: String,
    },
    /// Timeout on all submissions (simulate slow response)
    Timeout { delay_ms: u64 },
    /// Return 503 Service Unavailable
    ServiceUnavailable,
    /// Return 429 Too Many Requests (rate limiting)
    RateLimited,
    /// Intermittent failures (random based on rate)
    Intermittent { failure_rate: f64 },
    /// Partial rejection (accept some records, reject others)
    PartialReject { reject_ratio: f64 },
    /// Queue mode (accept but return pending status)
    Queued { queue_delay_ms: u64 },
    /// Custom response
    Custom {
        status_code: u16,
        body: String,
        headers: HashMap<String, String>,
    },
}

impl Default for RegulatorMode {
    fn default() -> Self {
        Self::Accept
    }
}

/// Off-peak hours configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffPeakConfig {
    /// Whether off-peak restrictions are enabled
    pub enabled: bool,
    /// Off-peak start hour (0-23)
    pub start_hour: u8,
    /// Off-peak end hour (0-23)
    pub end_hour: u8,
    /// Timezone offset from UTC (hours)
    pub timezone_offset: i8,
    /// Whether to reject submissions outside off-peak hours
    pub reject_outside_window: bool,
}

impl Default for OffPeakConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            start_hour: 22,  // 10 PM
            end_hour: 6,     // 6 AM
            timezone_offset: 7, // WIB (UTC+7)
            reject_outside_window: false,
        }
    }
}

impl OffPeakConfig {
    /// Check if current time is within off-peak window
    pub fn is_off_peak_now(&self) -> bool {
        if !self.enabled {
            return true; // If not enabled, always allow
        }

        let now = chrono::Utc::now();
        let local_hour = ((now.time().hour() as i8 + self.timezone_offset) % 24) as u8;

        if self.start_hour > self.end_hour {
            // Window crosses midnight (e.g., 22:00 - 06:00)
            local_hour >= self.start_hour || local_hour < self.end_hour
        } else {
            // Window within same day (e.g., 02:00 - 06:00)
            local_hour >= self.start_hour && local_hour < self.end_hour
        }
    }
}

// ============================================================================
// Latency Configuration
// ============================================================================

/// Latency simulation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyConfig {
    /// Whether latency simulation is enabled
    pub enabled: bool,
    /// Base latency in milliseconds
    pub base_ms: u64,
    /// Random jitter range (+/- ms)
    pub jitter_ms: u64,
    /// Percentile latencies (p50, p90, p99)
    pub percentiles: Option<LatencyPercentiles>,
}

impl Default for LatencyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_ms: 0,
            jitter_ms: 0,
            percentiles: None,
        }
    }
}

impl LatencyConfig {
    /// No latency (for fast tests)
    pub fn none() -> Self {
        Self::default()
    }

    /// Minimal latency (1-5ms)
    pub fn minimal() -> Self {
        Self {
            enabled: true,
            base_ms: 1,
            jitter_ms: 2,
            percentiles: None,
        }
    }

    /// Realistic latency (10-50ms with occasional spikes)
    pub fn realistic() -> Self {
        Self {
            enabled: true,
            base_ms: 20,
            jitter_ms: 15,
            percentiles: Some(LatencyPercentiles {
                p50_ms: 20,
                p90_ms: 50,
                p99_ms: 150,
            }),
        }
    }

    /// High latency (for timeout testing)
    pub fn high(base_ms: u64) -> Self {
        Self {
            enabled: true,
            base_ms,
            jitter_ms: base_ms / 10,
            percentiles: None,
        }
    }

    /// Calculate actual latency to apply
    pub fn calculate_latency(&self) -> Duration {
        if !self.enabled {
            return Duration::ZERO;
        }

        let base = self.base_ms as f64;
        let jitter = if self.jitter_ms > 0 {
            let jitter_range = self.jitter_ms as f64;
            (rand::random::<f64>() * 2.0 - 1.0) * jitter_range
        } else {
            0.0
        };

        // Apply percentile-based latency if configured
        if let Some(ref percentiles) = self.percentiles {
            let roll: f64 = rand::random();
            let latency_ms = if roll > 0.99 {
                percentiles.p99_ms as f64
            } else if roll > 0.90 {
                percentiles.p90_ms as f64
            } else {
                percentiles.p50_ms as f64
            };
            Duration::from_millis((latency_ms + jitter).max(0.0) as u64)
        } else {
            Duration::from_millis((base + jitter).max(0.0) as u64)
        }
    }

    /// Apply latency (sleep for calculated duration)
    pub async fn apply(&self) {
        let duration = self.calculate_latency();
        if !duration.is_zero() {
            tokio::time::sleep(duration).await;
        }
    }
}

/// Latency percentiles
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyPercentiles {
    pub p50_ms: u64,
    pub p90_ms: u64,
    pub p99_ms: u64,
}

// ============================================================================
// Failure Injection Configuration
// ============================================================================

/// Failure injection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureInjectionConfig {
    /// Whether failure injection is enabled
    pub enabled: bool,
    /// Probability of failure (0.0 - 1.0)
    pub failure_rate: f64,
    /// Types of failures to inject
    pub failure_types: Vec<FailureType>,
}

impl Default for FailureInjectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            failure_rate: 0.0,
            failure_types: vec![FailureType::InternalError],
        }
    }
}

impl FailureInjectionConfig {
    /// No failures
    pub fn none() -> Self {
        Self::default()
    }

    /// Low failure rate (1%)
    pub fn low() -> Self {
        Self {
            enabled: true,
            failure_rate: 0.01,
            failure_types: vec![FailureType::InternalError, FailureType::Timeout],
        }
    }

    /// Medium failure rate (5%)
    pub fn medium() -> Self {
        Self {
            enabled: true,
            failure_rate: 0.05,
            failure_types: vec![
                FailureType::InternalError,
                FailureType::Timeout,
                FailureType::ServiceUnavailable,
            ],
        }
    }

    /// High failure rate (20%) - for chaos testing
    pub fn high() -> Self {
        Self {
            enabled: true,
            failure_rate: 0.20,
            failure_types: vec![
                FailureType::InternalError,
                FailureType::Timeout,
                FailureType::ServiceUnavailable,
                FailureType::ConnectionReset,
            ],
        }
    }

    /// Check if a failure should be injected
    pub fn should_fail(&self) -> bool {
        self.enabled && rand::random::<f64>() < self.failure_rate
    }

    /// Get a random failure type
    pub fn random_failure(&self) -> Option<&FailureType> {
        if self.should_fail() && !self.failure_types.is_empty() {
            let idx = rand::random::<usize>() % self.failure_types.len();
            Some(&self.failure_types[idx])
        } else {
            None
        }
    }
}

/// Types of failures that can be injected
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureType {
    /// HTTP 500 Internal Server Error
    InternalError,
    /// Request timeout
    Timeout,
    /// HTTP 503 Service Unavailable
    ServiceUnavailable,
    /// Connection reset
    ConnectionReset,
    /// HTTP 429 Too Many Requests
    RateLimited,
    /// Malformed response
    MalformedResponse,
    /// Partial response
    PartialResponse,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SimulatorConfig::default();
        assert!(config.core_banking.enabled);
        assert!(config.mapping_service.enabled);
        assert!(config.rulepack_service.enabled);
        assert!(config.regulator_endpoint.enabled);
    }

    #[test]
    fn test_config_builder() {
        let config = SimulatorConfig::builder()
            .core_banking(CoreBankingConfig {
                port: 9001,
                ..Default::default()
            })
            .build();

        assert_eq!(config.core_banking.port, 9001);
        assert_eq!(config.mapping_service.port, 18082); // Default
    }

    #[test]
    fn test_latency_config_none() {
        let latency = LatencyConfig::none();
        assert!(!latency.enabled);
        assert_eq!(latency.calculate_latency(), Duration::ZERO);
    }

    #[test]
    fn test_latency_config_minimal() {
        let latency = LatencyConfig::minimal();
        assert!(latency.enabled);
        let duration = latency.calculate_latency();
        assert!(duration.as_millis() <= 10);
    }

    #[test]
    fn test_failure_injection() {
        let config = FailureInjectionConfig {
            enabled: true,
            failure_rate: 1.0, // Always fail
            failure_types: vec![FailureType::InternalError],
        };
        assert!(config.should_fail());
        assert_eq!(config.random_failure(), Some(&FailureType::InternalError));
    }

    #[test]
    fn test_failure_injection_disabled() {
        let config = FailureInjectionConfig::none();
        assert!(!config.should_fail());
        assert!(config.random_failure().is_none());
    }

    #[test]
    fn test_regulator_mode_timeout() {
        let config = RegulatorEndpointConfig::default().timeout_mode(5000);
        match config.mode {
            RegulatorMode::Timeout { delay_ms } => assert_eq!(delay_ms, 5000),
            _ => panic!("Expected Timeout mode"),
        }
    }

    #[test]
    fn test_regulator_mode_reject() {
        let config = RegulatorEndpointConfig::default()
            .reject_mode("ERR001", "Invalid submission");
        match config.mode {
            RegulatorMode::Reject { error_code, error_message } => {
                assert_eq!(error_code, "ERR001");
                assert_eq!(error_message, "Invalid submission");
            }
            _ => panic!("Expected Reject mode"),
        }
    }

    #[test]
    fn test_off_peak_window() {
        let config = OffPeakConfig {
            enabled: true,
            start_hour: 22,
            end_hour: 6,
            timezone_offset: 0,
            reject_outside_window: false,
        };
        // This is a time-dependent test, just verify it doesn't panic
        let _ = config.is_off_peak_now();
    }

    #[test]
    fn test_socket_addr() {
        let config = CoreBankingConfig::default();
        assert_eq!(config.socket_addr(), "127.0.0.1:18081");
    }

    #[test]
    fn test_dirty_ratio_clamping() {
        let config = CoreBankingConfig::default().with_dirty_ratio(1.5);
        assert_eq!(config.default_dirty_ratio, 1.0);

        let config = CoreBankingConfig::default().with_dirty_ratio(-0.5);
        assert_eq!(config.default_dirty_ratio, 0.0);
    }

    #[test]
    fn test_config_for_ci() {
        let config = SimulatorConfig::for_ci();
        assert!(!config.core_banking.latency.enabled);
        assert!(!config.mapping_service.latency.enabled);
    }

    #[test]
    fn test_config_for_load_test() {
        let config = SimulatorConfig::for_load_test();
        assert!(config.core_banking.latency.enabled);
        assert!(config.regulator_endpoint.latency.enabled);
    }
}
