//! # Credit Data Simulator
//!
//! Multi-service credit data pipeline simulator for testing and benchmarking
//! [VIL](https://github.com/OceanOS-id/VIL) NDJSON pipeline examples.
//! Also usable as a standalone mock for any credit/banking data integration testing.
//!
//! ## Services
//!
//! - **CoreBankingSimulator** (:18081) — Credit records with NDJSON bulk export, pagination, dirty data ratio
//! - **MappingServiceSimulator** (:18082) — Versioned SLIK field mapping configurations
//! - **RulepackServiceSimulator** (:18083) — Versioned validation rules engine
//! - **RegulatorEndpointSimulator** (:18084) — OJK regulatory submission endpoint
//!
//! ## Usage
//!
//! ```rust,ignore
//! use credit_data_simulator::{SimulatorServer, SimulatorConfig};
//!
//! #[tokio::test]
//! async fn test_with_simulators() {
//!     let config = SimulatorConfig::default();
//!     let server = SimulatorServer::start(config).await.unwrap();
//!     // Run tests...
//!     server.shutdown().await;
//! }
//! ```

pub mod config;
pub mod core_banking;
pub mod engine;
pub mod mapping_service;
pub mod models;
pub mod regulator_endpoint;
pub mod rulepack_service;

pub use config::*;
pub use core_banking::CoreBankingSimulator;
pub use engine::{EngineSimulator, AdminSimulator};
pub use mapping_service::MappingServiceSimulator;
pub use regulator_endpoint::RegulatorEndpointSimulator;
pub use rulepack_service::RulepackServiceSimulator;

use std::collections::HashMap;

use std::sync::Arc;
use tokio::sync::{oneshot, RwLock};

// ============================================================================
// Common Types
// ============================================================================

/// Result type for simulator operations
pub type SimulatorResult<T> = Result<T, SimulatorError>;

/// Errors that can occur in simulators
#[derive(Debug, Clone)]
pub enum SimulatorError {
    /// Server failed to bind to address
    BindError(String),
    /// Server failed to start
    StartError(String),
    /// Configuration error
    ConfigError(String),
    /// Internal error
    InternalError(String),
    /// Simulator not found
    NotFound(String),
    /// Timeout occurred
    Timeout,
    /// Service unavailable (simulated)
    ServiceUnavailable,
}

impl std::fmt::Display for SimulatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BindError(msg) => write!(f, "Bind error: {}", msg),
            Self::StartError(msg) => write!(f, "Start error: {}", msg),
            Self::ConfigError(msg) => write!(f, "Config error: {}", msg),
            Self::InternalError(msg) => write!(f, "Internal error: {}", msg),
            Self::NotFound(msg) => write!(f, "Not found: {}", msg),
            Self::Timeout => write!(f, "Timeout"),
            Self::ServiceUnavailable => write!(f, "Service unavailable"),
        }
    }
}

impl std::error::Error for SimulatorError {}

/// Health status of a simulator
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthStatus {
    pub service: String,
    pub status: String,
    pub version: String,
    pub uptime_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl HealthStatus {
    pub fn healthy(service: &str, version: &str, uptime_secs: u64) -> Self {
        Self {
            service: service.to_string(),
            status: "healthy".to_string(),
            version: version.to_string(),
            uptime_secs,
            details: None,
        }
    }

    pub fn unhealthy(service: &str, reason: &str) -> Self {
        let mut details = serde_json::Map::new();
        details.insert(
            "reason".to_string(),
            serde_json::Value::String(reason.to_string()),
        );
        Self {
            service: service.to_string(),
            status: "unhealthy".to_string(),
            version: "unknown".to_string(),
            uptime_secs: 0,
            details: Some(serde_json::Value::Object(details)),
        }
    }

    pub fn with_details(mut self, key: &str, value: serde_json::Value) -> Self {
        if self.details.is_none() {
            self.details = Some(serde_json::Value::Object(serde_json::Map::new()));
        }

        if let Some(serde_json::Value::Object(map)) = &mut self.details {
            map.insert(key.to_string(), value);
        }
        self
    }
}

/// Statistics tracked by simulators
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct SimulatorStats {
    pub requests_total: u64,
    pub requests_success: u64,
    pub requests_failed: u64,
    pub requests_timeout: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub total_latency_ms: f64,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub endpoint_counts: HashMap<String, u64>,
}

impl serde::Serialize for SimulatorStats {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("SimulatorStats", 8)?;
        state.serialize_field("requests_total", &self.requests_total)?;
        state.serialize_field("requests_success", &self.requests_success)?;
        state.serialize_field("requests_failed", &self.requests_failed)?;
        state.serialize_field("requests_timeout", &self.requests_timeout)?;
        state.serialize_field("bytes_sent", &self.bytes_sent)?;
        state.serialize_field("bytes_received", &self.bytes_received)?;
        state.serialize_field("avg_latency_ms", &self.avg_latency_ms())?;
        if !self.endpoint_counts.is_empty() {
            state.serialize_field("endpoint_counts", &self.endpoint_counts)?;
        }
        state.end()
    }
}

impl SimulatorStats {
    pub fn avg_latency_ms(&self) -> f64 {
        if self.requests_total > 0 {
            self.total_latency_ms / self.requests_total as f64
        } else {
            0.0
        }
    }

    pub fn record_request(&mut self, endpoint: &str, success: bool, latency_ms: f64) {
        self.requests_total += 1;
        if success {
            self.requests_success += 1;
        } else {
            self.requests_failed += 1;
        }

        self.total_latency_ms += latency_ms;

        *self.endpoint_counts.entry(endpoint.to_string()).or_insert(0) += 1;
    }

    pub fn record_timeout(&mut self) {
        self.requests_total += 1;
        self.requests_timeout += 1;
    }

    pub fn record_bytes(&mut self, sent: u64, received: u64) {
        self.bytes_sent += sent;
        self.bytes_received += received;
    }
}

// ============================================================================
// Simulator Trait
// ============================================================================

/// Trait for all simulators
#[async_trait::async_trait]
pub trait Simulator: Send + Sync {
    /// Get the name of this simulator
    fn name(&self) -> &str;

    /// Get the port this simulator is running on
    fn port(&self) -> u16;

    /// Get the base URL for this simulator
    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }

    /// Get health status
    async fn health(&self) -> HealthStatus;

    /// Get statistics
    async fn stats(&self) -> SimulatorStats;

    /// Reset statistics
    async fn reset_stats(&self);

    /// Check if the simulator is ready to accept requests
    async fn is_ready(&self) -> bool {
        self.health().await.status == "healthy"
    }
}

// ============================================================================
// Simulator Server (Unified)
// ============================================================================

/// Manages all simulators as a unified server
pub struct SimulatorServer {
    config: SimulatorConfig,
    core_banking: Option<Arc<CoreBankingSimulator>>,
    mapping_service: Option<Arc<MappingServiceSimulator>>,
    rulepack_service: Option<Arc<RulepackServiceSimulator>>,
    regulator_endpoint: Option<Arc<RegulatorEndpointSimulator>>,
    shutdown_txs: Vec<oneshot::Sender<()>>,
    started_at: std::time::Instant,
}

impl SimulatorServer {
    /// Create a new simulator server with the given configuration
    pub fn new(config: SimulatorConfig) -> Self {
        Self {
            config,
            core_banking: None,
            mapping_service: None,
            rulepack_service: None,
            regulator_endpoint: None,
            shutdown_txs: Vec::new(),
            started_at: std::time::Instant::now(),
        }
    }

    /// Start all configured simulators
    pub async fn start(config: SimulatorConfig) -> SimulatorResult<Self> {
        let mut server = Self::new(config.clone());

        // Start Core Banking Simulator
        if config.core_banking.enabled {
            let simulator = CoreBankingSimulator::new(config.core_banking.clone());
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let sim = Arc::new(simulator);
            let sim_clone = sim.clone();

            tokio::spawn(async move {
                if let Err(e) = sim_clone.run(shutdown_rx).await {
                    tracing::error!("Core Banking Simulator error: {}", e);
                }
            });

            server.core_banking = Some(sim);
            server.shutdown_txs.push(shutdown_tx);
            tracing::info!(
                "Core Banking Simulator started on port {}",
                config.core_banking.port
            );
        }

        // Start Mapping Service Simulator
        if config.mapping_service.enabled {
            let simulator = MappingServiceSimulator::new(config.mapping_service.clone());
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let sim = Arc::new(simulator);
            let sim_clone = sim.clone();

            tokio::spawn(async move {
                if let Err(e) = sim_clone.run(shutdown_rx).await {
                    tracing::error!("Mapping Service Simulator error: {}", e);
                }
            });

            server.mapping_service = Some(sim);
            server.shutdown_txs.push(shutdown_tx);
            tracing::info!(
                "Mapping Service Simulator started on port {}",
                config.mapping_service.port
            );
        }

        // Start Rulepack Service Simulator
        if config.rulepack_service.enabled {
            let simulator = RulepackServiceSimulator::new(config.rulepack_service.clone());
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let sim = Arc::new(simulator);
            let sim_clone = sim.clone();

            tokio::spawn(async move {
                if let Err(e) = sim_clone.run(shutdown_rx).await {
                    tracing::error!("Rulepack Service Simulator error: {}", e);
                }
            });

            server.rulepack_service = Some(sim);
            server.shutdown_txs.push(shutdown_tx);
            tracing::info!(
                "Rulepack Service Simulator started on port {}",
                config.rulepack_service.port
            );
        }

        // Start Regulator Endpoint Simulator
        if config.regulator_endpoint.enabled {
            let simulator = RegulatorEndpointSimulator::new(config.regulator_endpoint.clone());
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let sim = Arc::new(simulator);
            let sim_clone = sim.clone();

            tokio::spawn(async move {
                if let Err(e) = sim_clone.run(shutdown_rx).await {
                    tracing::error!("Regulator Endpoint Simulator error: {}", e);
                }
            });

            server.regulator_endpoint = Some(sim);
            server.shutdown_txs.push(shutdown_tx);
            tracing::info!(
                "Regulator Endpoint Simulator started on port {}",
                config.regulator_endpoint.port
            );
        }

        // Wait for all simulators to be ready
        server.wait_for_ready(std::time::Duration::from_secs(10)).await?;

        Ok(server)
    }

    /// Wait for all simulators to be ready
    pub async fn wait_for_ready(&self, timeout: std::time::Duration) -> SimulatorResult<()> {
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                return Err(SimulatorError::Timeout);
            }

            let mut all_ready = true;

            if let Some(ref sim) = self.core_banking {
                if !sim.is_ready().await {
                    all_ready = false;
                }
            }
            if let Some(ref sim) = self.mapping_service {
                if !sim.is_ready().await {
                    all_ready = false;
                }
            }
            if let Some(ref sim) = self.rulepack_service {
                if !sim.is_ready().await {
                    all_ready = false;
                }
            }
            if let Some(ref sim) = self.regulator_endpoint {
                if !sim.is_ready().await {
                    all_ready = false;
                }
            }

            if all_ready {
                return Ok(());
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Shutdown all simulators
    pub async fn shutdown(self) {
        for tx in self.shutdown_txs {
            let _ = tx.send(());
        }
        tracing::info!("All simulators shut down");
    }

    /// Get the Core Banking simulator
    pub fn core_banking(&self) -> Option<&Arc<CoreBankingSimulator>> {
        self.core_banking.as_ref()
    }

    /// Get the Mapping Service simulator
    pub fn mapping_service(&self) -> Option<&Arc<MappingServiceSimulator>> {
        self.mapping_service.as_ref()
    }

    /// Get the Rulepack Service simulator
    pub fn rulepack_service(&self) -> Option<&Arc<RulepackServiceSimulator>> {
        self.rulepack_service.as_ref()
    }

    /// Get the Regulator Endpoint simulator
    pub fn regulator_endpoint(&self) -> Option<&Arc<RegulatorEndpointSimulator>> {
        self.regulator_endpoint.as_ref()
    }

    /// Get uptime in seconds
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// Get combined statistics from all simulators
    pub async fn combined_stats(&self) -> HashMap<String, SimulatorStats> {
        let mut stats = HashMap::new();

        if let Some(ref sim) = self.core_banking {
            stats.insert(sim.name().to_string(), sim.stats().await);
        }
        if let Some(ref sim) = self.mapping_service {
            stats.insert(sim.name().to_string(), sim.stats().await);
        }
        if let Some(ref sim) = self.rulepack_service {
            stats.insert(sim.name().to_string(), sim.stats().await);
        }
        if let Some(ref sim) = self.regulator_endpoint {
            stats.insert(sim.name().to_string(), sim.stats().await);
        }

        stats
    }

    /// Reset all statistics
    pub async fn reset_all_stats(&self) {
        if let Some(ref sim) = self.core_banking {
            sim.reset_stats().await;
        }
        if let Some(ref sim) = self.mapping_service {
            sim.reset_stats().await;
        }
        if let Some(ref sim) = self.rulepack_service {
            sim.reset_stats().await;
        }
        if let Some(ref sim) = self.regulator_endpoint {
            sim.reset_stats().await;
        }
    }

    /// Get endpoint URLs for all simulators
    pub fn endpoints(&self) -> SimulatorEndpoints {
        SimulatorEndpoints {
            core_banking: self.core_banking.as_ref().map(|s| s.base_url()),
            mapping_service: self.mapping_service.as_ref().map(|s| s.base_url()),
            rulepack_service: self.rulepack_service.as_ref().map(|s| s.base_url()),
            regulator_endpoint: self.regulator_endpoint.as_ref().map(|s| s.base_url()),
        }
    }
}

/// Endpoint URLs for all simulators
#[derive(Debug, Clone, serde::Serialize)]
pub struct SimulatorEndpoints {
    pub core_banking: Option<String>,
    pub mapping_service: Option<String>,
    pub rulepack_service: Option<String>,
    pub regulator_endpoint: Option<String>,
}

// ============================================================================
// Test Utilities
// ============================================================================

/// Quick start helper for tests - starts all simulators with default config
pub async fn start_all_simulators() -> SimulatorResult<SimulatorServer> {
    let config = SimulatorConfig::default();
    SimulatorServer::start(config).await
}

/// Quick start helper for tests - starts simulators with custom ports
pub async fn start_simulators_on_ports(
    core_banking_port: u16,
    mapping_port: u16,
    rulepack_port: u16,
    regulator_port: u16,
) -> SimulatorResult<SimulatorServer> {
    let mut config = SimulatorConfig::default();
    config.core_banking.port = core_banking_port;
    config.mapping_service.port = mapping_port;
    config.rulepack_service.port = rulepack_port;
    config.regulator_endpoint.port = regulator_port;
    SimulatorServer::start(config).await
}

/// Find available ports for simulators
pub async fn find_available_ports(count: usize) -> Vec<u16> {
    let mut ports = Vec::with_capacity(count);
    for _ in 0..count {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        ports.push(port);
        drop(listener);
    }
    ports
}

// ============================================================================
// Shared State Types
// ============================================================================

/// Thread-safe wrapper for simulator state
pub type SharedState<T> = Arc<RwLock<T>>;

/// Create a new shared state
pub fn shared_state<T>(value: T) -> SharedState<T> {
    Arc::new(RwLock::new(value))
}

// ============================================================================
// HTTP Response Helpers
// ============================================================================

/// Standard JSON response wrapper
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<ResponseMeta>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            meta: None,
        }
    }

    pub fn success_with_meta(data: T, meta: ResponseMeta) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            meta: Some(meta),
        }
    }

    pub fn error(code: &str, message: &str) -> ApiResponse<T> {
        ApiResponse {
            success: false,
            data: None,
            error: Some(ApiError {
                code: code.to_string(),
                message: message.to_string(),
                details: None,
            }),
            meta: None,
        }
    }
}

/// API error details
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Response metadata for pagination, timing, etc.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResponseMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_pages: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing_time_ms: Option<u64>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub extra: Option<HashMap<String, serde_json::Value>>,
}

impl ResponseMeta {
    pub fn paginated(page: u32, page_size: u32, total_count: u64) -> Self {
        let total_pages = if page_size == 0 {
            0
        } else {
            ((total_count + page_size as u64 - 1) / page_size as u64) as u32
        };
        Self {
            page: Some(page),
            page_size: Some(page_size),
            total_count: Some(total_count),
            total_pages: Some(total_pages),
            processing_time_ms: None,
            extra: None,
        }
    }

    pub fn with_timing(mut self, ms: u64) -> Self {
        self.processing_time_ms = Some(ms);
        self
    }

    pub fn with_extra(mut self, key: &str, value: serde_json::Value) -> Self {
        if self.extra.is_none() {
            self.extra = Some(HashMap::new());
        }
        if let Some(map) = &mut self.extra {
            map.insert(key.to_string(), value);
        }
        self
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_healthy() {
        let status = HealthStatus::healthy("test-service", "1.0.0", 100);
        assert_eq!(status.status, "healthy");
        assert_eq!(status.service, "test-service");
        assert_eq!(status.version, "1.0.0");
        assert_eq!(status.uptime_secs, 100);
    }

    #[test]
    fn test_health_status_unhealthy() {
        let status = HealthStatus::unhealthy("test-service", "connection failed");
        assert_eq!(status.status, "unhealthy");
        assert!(status.details.is_some());
    }

    #[test]
    fn test_simulator_stats_record() {
        let mut stats = SimulatorStats::default();
        stats.record_request("/api/test", true, 10.0);
        stats.record_request("/api/test", true, 20.0);
        stats.record_request("/api/other", false, 30.0);

        assert_eq!(stats.requests_total, 3);
        assert_eq!(stats.requests_success, 2);
        assert_eq!(stats.requests_failed, 1);
        assert_eq!(stats.endpoint_counts.get("/api/test"), Some(&2));
        assert_eq!(stats.endpoint_counts.get("/api/other"), Some(&1));
    }

    #[test]
    fn test_api_response_success() {
        let response: ApiResponse<String> = ApiResponse::success("test data".to_string());
        assert!(response.success);
        assert_eq!(response.data, Some("test data".to_string()));
        assert!(response.error.is_none());
    }

    #[test]
    fn test_api_response_error() {
        let response: ApiResponse<String> = ApiResponse::error("ERR001", "Something went wrong");
        assert!(!response.success);
        assert!(response.data.is_none());
        assert!(response.error.is_some());
        assert_eq!(response.error.as_ref().unwrap().code, "ERR001");
    }

    #[test]
    fn test_response_meta_paginated() {
        let meta = ResponseMeta::paginated(1, 10, 95);
        assert_eq!(meta.page, Some(1));
        assert_eq!(meta.page_size, Some(10));
        assert_eq!(meta.total_count, Some(95));
        assert_eq!(meta.total_pages, Some(10)); // ceil(95/10) = 10
    }
}
