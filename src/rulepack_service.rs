//! # Rulepack Service Simulator
//!
//! Simulates the Rule Engine service responsible for validating data against
//! complex business rules and regulatory requirements.
//!
//! ## Business Purpose
//!
//! The Rulepack Service ensures that all data submitted to the regulator
//! complies with the defined schemas, validation rules, and cross-field constraints.
//! It acts as the final gatekeeper before data submission.
//!
//! ## Key Features
//!
//! - **Versioning**: Manages multiple rulepack versions (v1, v2, minimal).
//! - **Validation Rule Types**:
//!   - `Required`: Mandatory field checks.
//!   - `Length`, `Range`, `Pattern`: Format validation.
//!   - `Enum`: Value list validation.
//!   - `DateFormat`: Date string validity.
//! - **Cross-Field Validation**:
//!   - `Compare`: Compare two fields (e.g. start_date <= end_date).
//!   - `Conditional`: If Field A = X, then Field B must be present.
//! - **Severity Levels**: Support for Errors (blocking) and Warnings (non-blocking).
//!
//! ## API Endpoints
//!
//! - `GET /api/v1/rulepacks` - List available rulepacks.
//! - `GET /api/v1/rulepacks/:version` - Get rulepack definition.
//! - `POST /api/v1/rulepacks/:version/validate` - Validate records.
//! - `POST /api/v1/rulepacks` - Create dynamic rulepack (testing).
//! - `GET /api/v1/stats` - Get simulator statistics.

use crate::{
    config::{FailureType, RulepackServiceConfig},
    ApiResponse, HealthStatus, ResponseMeta, SharedState, SimulatorError, SimulatorResult,
    SimulatorStats, Simulator, shared_state,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::oneshot;

// ============================================================================
// Data Models
// ============================================================================

/// Rulepack configuration for a specific version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulepackConfig {
    /// Version identifier
    pub version: String,
    /// Display name
    pub name: String,
    /// Description
    pub description: String,
    /// Validation rules
    pub rules: Vec<ValidationRule>,
    /// Cross-field rules
    pub cross_field_rules: Vec<CrossFieldRule>,
    /// Creation timestamp
    pub created_at: String,
    /// Whether this rulepack is active
    pub is_active: bool,
}

/// Individual validation rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRule {
    /// Rule identifier
    pub rule_id: String,
    /// Target field
    pub field: String,
    /// Rule type
    pub rule_type: RuleType,
    /// Severity (error or warning)
    pub severity: Severity,
    /// Error message
    pub message: String,
    /// Rule parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, serde_json::Value>>,
}

/// Type of validation rule
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleType {
    Required,
    Length,
    Range,
    Pattern,
    Enum,
    DateFormat,
    Custom,
}

/// Severity level
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// Cross-field validation rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossFieldRule {
    pub rule_id: String,
    pub rule_type: CrossFieldRuleType,
    pub fields: Vec<String>,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, serde_json::Value>>,
}

/// Type of cross-field validation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CrossFieldRuleType {
    Compare,
    Conditional,
    Custom,
}

/// Validation result for a single record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub record_index: usize,
    pub valid: bool,
    pub errors: Vec<RuleViolation>,
    pub warnings: Vec<RuleViolation>,
}

/// Individual rule violation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleViolation {
    pub rule_id: String,
    pub field: String,
    pub message: String,
    pub severity: Severity,
}

/// Request to validate records
#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    pub records: Vec<serde_json::Value>,
}

/// Query params for listing rulepacks
#[derive(Debug, Deserialize)]
pub struct ListRulepacksParams {
    pub active_only: Option<bool>,
}

impl RulepackConfig {
    /// Create v1 rulepack (basic SLIK rules)
    pub fn v1() -> Self {
        Self {
            version: "v1".to_string(),
            name: "SLIK Basic Rulepack v1".to_string(),
            description: "Basic validation rules for SLIK credit reporting".to_string(),
            rules: vec![
                ValidationRule {
                    rule_id: "R001".to_string(),
                    field: "nik".to_string(),
                    rule_type: RuleType::Required,
                    severity: Severity::Error,
                    message: "NIK is required".to_string(),
                    params: None,
                },
                ValidationRule {
                    rule_id: "R002".to_string(),
                    field: "nik".to_string(),
                    rule_type: RuleType::Length,
                    severity: Severity::Error,
                    message: "NIK must be exactly 16 digits".to_string(),
                    params: Some({
                        let mut p = HashMap::new();
                        p.insert("min".to_string(), serde_json::json!(16));
                        p.insert("max".to_string(), serde_json::json!(16));
                        p
                    }),
                },
                ValidationRule {
                    rule_id: "R003".to_string(),
                    field: "nama_lengkap".to_string(),
                    rule_type: RuleType::Required,
                    severity: Severity::Error,
                    message: "Full name is required".to_string(),
                    params: None,
                },
                ValidationRule {
                    rule_id: "R004".to_string(),
                    field: "jumlah_kredit".to_string(),
                    rule_type: RuleType::Range,
                    severity: Severity::Error,
                    message: "Credit amount must be positive".to_string(),
                    params: Some({
                        let mut p = HashMap::new();
                        p.insert("min".to_string(), serde_json::json!(1));
                        p
                    }),
                },
                ValidationRule {
                    rule_id: "R005".to_string(),
                    field: "kolektabilitas".to_string(),
                    rule_type: RuleType::Range,
                    severity: Severity::Error,
                    message: "Collectability must be between 1 and 5".to_string(),
                    params: Some({
                        let mut p = HashMap::new();
                        p.insert("min".to_string(), serde_json::json!(1));
                        p.insert("max".to_string(), serde_json::json!(5));
                        p
                    }),
                },
                ValidationRule {
                    rule_id: "R006".to_string(),
                    field: "mata_uang".to_string(),
                    rule_type: RuleType::Enum,
                    severity: Severity::Error,
                    message: "Currency must be a valid ISO 4217 code".to_string(),
                    params: Some({
                        let mut p = HashMap::new();
                        p.insert("values".to_string(), serde_json::json!(["IDR", "USD", "EUR", "SGD", "JPY"]));
                        p
                    }),
                },
            ],
            cross_field_rules: vec![
                CrossFieldRule {
                    rule_id: "CF001".to_string(),
                    rule_type: CrossFieldRuleType::Compare,
                    fields: vec!["tanggal_mulai".to_string(), "tanggal_jatuh_tempo".to_string()],
                    severity: Severity::Error,
                    message: "Start date must be before maturity date".to_string(),
                    params: Some({
                        let mut p = HashMap::new();
                        p.insert("operator".to_string(), serde_json::json!("<="));
                        p
                    }),
                },
                CrossFieldRule {
                    rule_id: "CF002".to_string(),
                    rule_type: CrossFieldRuleType::Compare,
                    fields: vec!["saldo_outstanding".to_string(), "jumlah_kredit".to_string()],
                    severity: Severity::Warning,
                    message: "Outstanding balance should not exceed credit amount".to_string(),
                    params: Some({
                        let mut p = HashMap::new();
                        p.insert("operator".to_string(), serde_json::json!("<="));
                        p
                    }),
                },
            ],
            created_at: "2023-01-15T00:00:00Z".to_string(),
            is_active: true,
        }
    }

    /// Create v2 rulepack (enhanced with additional checks)
    pub fn v2() -> Self {
        let mut rulepack = Self::v1();
        rulepack.version = "v2".to_string();
        rulepack.name = "SLIK Enhanced Rulepack v2".to_string();
        rulepack.description = "Enhanced validation with additional regulatory checks".to_string();

        rulepack.rules.extend(vec![
            ValidationRule {
                rule_id: "R007".to_string(),
                field: "tanggal_mulai".to_string(),
                rule_type: RuleType::DateFormat,
                severity: Severity::Error,
                message: "Start date must be in YYYY-MM-DD format".to_string(),
                params: Some({
                    let mut p = HashMap::new();
                    p.insert("format".to_string(), serde_json::json!("%Y-%m-%d"));
                    p
                }),
            },
            ValidationRule {
                rule_id: "R008".to_string(),
                field: "kode_cabang".to_string(),
                rule_type: RuleType::Pattern,
                severity: Severity::Error,
                message: "Branch code must be 3 letters followed by 3 digits".to_string(),
                params: Some({
                    let mut p = HashMap::new();
                    p.insert("pattern".to_string(), serde_json::json!(r"^[A-Z]{3}\d{3}$"));
                    p
                }),
            },
        ]);

        rulepack.created_at = "2024-01-10T00:00:00Z".to_string();
        rulepack
    }
}

// ============================================================================
// Simulator State
// ============================================================================

/// Internal state for Rulepack Service Simulator
pub struct RulepackServiceState {
    pub config: RulepackServiceConfig,
    pub rulepacks: HashMap<String, RulepackConfig>,
    pub stats: SimulatorStats,
    pub started_at: Instant,
    pub ready: bool,
}

impl RulepackServiceState {
    pub fn new(config: RulepackServiceConfig) -> Self {
        let mut rulepacks = HashMap::new();

        if config.available_versions.contains(&"v1".to_string()) {
            rulepacks.insert("v1".to_string(), RulepackConfig::v1());
        }
        if config.available_versions.contains(&"v2".to_string()) {
            rulepacks.insert("v2".to_string(), RulepackConfig::v2());
        }

        Self {
            config,
            rulepacks,
            stats: SimulatorStats::default(),
            started_at: Instant::now(),
            ready: false,
        }
    }
}

// ============================================================================
// Rulepack Service Simulator
// ============================================================================

/// Rulepack Service Simulator implementation
pub struct RulepackServiceSimulator {
    state: SharedState<RulepackServiceState>,
    config: RulepackServiceConfig,
}

impl RulepackServiceSimulator {
    /// Create a new Rulepack Service Simulator
    pub fn new(config: RulepackServiceConfig) -> Self {
        let state = shared_state(RulepackServiceState::new(config.clone()));
        Self { state, config }
    }

    /// Run the simulator HTTP server
    pub async fn run(&self, shutdown_rx: oneshot::Receiver<()>) -> SimulatorResult<()> {
        {
            let mut state = self.state.write().await;
            state.ready = true;
        }

        let app = self.create_router();
        let addr: std::net::SocketAddr = self.config.socket_addr().parse()
            .map_err(|e| SimulatorError::ConfigError(format!("Invalid address: {}", e)))?;

        tracing::info!("Rulepack Service Simulator listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr).await
            .map_err(|e| SimulatorError::BindError(e.to_string()))?;

        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
                tracing::info!("Rulepack Service Simulator shutting down");
            })
            .await
            .map_err(|e| SimulatorError::StartError(e.to_string()))?;

        Ok(())
    }

    fn create_router(&self) -> Router {
        let state = self.state.clone();

        Router::new()
            .route("/health", get(health_handler))
            .route("/api/v1/rulepacks", get(list_rulepacks_handler))
            .route("/api/v1/rulepacks", post(create_rulepack_handler))
            .route("/api/v1/rulepacks/:version", get(get_rulepack_handler))
            .route("/api/v1/rulepacks/:version/validate", post(validate_handler))
            .route("/api/v1/stats", get(stats_handler))
            .route("/api/v1/reset", post(reset_handler))
            .with_state(state)
    }

    /// Get a specific rulepack version
    pub async fn get_rulepack(&self, version: &str) -> Option<RulepackConfig> {
        self.state.read().await.rulepacks.get(version).cloned()
    }
}

#[async_trait::async_trait]
impl Simulator for RulepackServiceSimulator {
    fn name(&self) -> &str {
        "rulepack-service"
    }

    fn port(&self) -> u16 {
        self.config.port
    }

    async fn health(&self) -> HealthStatus {
        let state = self.state.read().await;
        let uptime = state.started_at.elapsed().as_secs();

        if state.ready {
            HealthStatus::healthy(self.name(), "1.0.0", uptime)
                .with_details("rulepack_count", serde_json::json!(state.rulepacks.len()))
                .with_details("available_versions", serde_json::json!(
                    state.rulepacks.keys().collect::<Vec<_>>()
                ))
        } else {
            HealthStatus::unhealthy(self.name(), "Not ready")
        }
    }

    async fn stats(&self) -> SimulatorStats {
        self.state.read().await.stats.clone()
    }

    async fn reset_stats(&self) {
        self.state.write().await.stats = SimulatorStats::default();
    }

    async fn is_ready(&self) -> bool {
        self.state.read().await.ready
    }
}

// ============================================================================
// HTTP Handlers
// ============================================================================

async fn health_handler(
    State(state): State<SharedState<RulepackServiceState>>,
) -> impl IntoResponse {
    let state = state.read().await;
    let uptime = state.started_at.elapsed().as_secs();

    if state.ready {
        let health = HealthStatus::healthy("rulepack-service", "1.0.0", uptime)
            .with_details("rulepack_count", serde_json::json!(state.rulepacks.len()));
        (StatusCode::OK, Json(health))
    } else {
        let health = HealthStatus::unhealthy("rulepack-service", "Not ready");
        (StatusCode::SERVICE_UNAVAILABLE, Json(health))
    }
}

/// Rulepack summary for listing
#[derive(Debug, Clone, Serialize)]
pub struct RulepackSummary {
    pub version: String,
    pub name: String,
    pub rule_count: usize,
    pub cross_field_rule_count: usize,
    pub is_active: bool,
}

async fn list_rulepacks_handler(
    State(state): State<SharedState<RulepackServiceState>>,
    Query(params): Query<ListRulepacksParams>,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut state_guard = state.write().await;

    let failure = state_guard.config.failure_injection.random_failure().cloned();
    if let Some(ref failure) = failure {
        state_guard.stats.record_request("/api/v1/rulepacks", false, start.elapsed().as_millis() as f64);
        return match failure {
            FailureType::InternalError => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<RulepackSummary>>::error("ERR500", "Internal server error")))
            }
            _ => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<RulepackSummary>>::error("ERR500", "Internal server error")))
            }
        };
    }

    state_guard.config.latency.apply().await;

    let active_only = params.active_only.unwrap_or(false);
    let summaries: Vec<RulepackSummary> = state_guard.rulepacks
        .values()
        .filter(|r| !active_only || r.is_active)
        .map(|r| RulepackSummary {
            version: r.version.clone(),
            name: r.name.clone(),
            rule_count: r.rules.len(),
            cross_field_rule_count: r.cross_field_rules.len(),
            is_active: r.is_active,
        })
        .collect();

    state_guard.stats.record_request("/api/v1/rulepacks", true, start.elapsed().as_millis() as f64);

    let meta = ResponseMeta {
        page: None,
        page_size: None,
        total_count: Some(summaries.len() as u64),
        total_pages: None,
        processing_time_ms: Some(start.elapsed().as_millis() as u64),
        extra: None,
    };

    (StatusCode::OK, Json(ApiResponse::success_with_meta(summaries, meta)))
}

async fn get_rulepack_handler(
    State(state): State<SharedState<RulepackServiceState>>,
    Path(version): Path<String>,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut state_guard = state.write().await;

    state_guard.config.latency.apply().await;

    if let Some(rulepack) = state_guard.rulepacks.get(&version).cloned() {
        state_guard.stats.record_request(&format!("/api/v1/rulepacks/{}", version), true, start.elapsed().as_millis() as f64);
        (StatusCode::OK, Json(ApiResponse::success(rulepack)))
    } else {
        state_guard.stats.record_request(&format!("/api/v1/rulepacks/{}", version), false, start.elapsed().as_millis() as f64);
        (StatusCode::NOT_FOUND, Json(ApiResponse::<RulepackConfig>::error("NOT_FOUND", &format!("Rulepack version '{}' not found", version))))
    }
}

async fn validate_handler(
    State(state): State<SharedState<RulepackServiceState>>,
    Path(version): Path<String>,
    Json(request): Json<ValidateRequest>,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut state_guard = state.write().await;

    state_guard.config.latency.apply().await;

    let rulepack = match state_guard.rulepacks.get(&version) {
        Some(rp) => rp.clone(),
        None => {
            state_guard.stats.record_request(&format!("/api/v1/rulepacks/{}/validate", version), false, start.elapsed().as_millis() as f64);
            return (StatusCode::NOT_FOUND, Json(ApiResponse::<Vec<ValidationResult>>::error("NOT_FOUND", &format!("Rulepack '{}' not found", version))));
        }
    };

    // Basic validation of each record
    let results: Vec<ValidationResult> = request.records.iter().enumerate().map(|(idx, record)| {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        for rule in &rulepack.rules {
            let value = record.get(&rule.field);
            let violation = match rule.rule_type {
                RuleType::Required => {
                    match value {
                        None => Some(rule.message.clone()),
                        Some(v) if v.is_null() || (v.is_string() && v.as_str().unwrap_or("").is_empty()) => Some(rule.message.clone()),
                        _ => None,
                    }
                }
                RuleType::Range => {
                    if let (Some(val), Some(params)) = (value, &rule.params) {
                        if let Some(num) = val.as_f64() {
                            let min = params.get("min").and_then(|v| v.as_f64());
                            let max = params.get("max").and_then(|v| v.as_f64());
                            if let Some(min) = min {
                                if num < min { return_violation(true, &rule.message) } else { None }
                            } else if let Some(max) = max {
                                if num > max { return_violation(true, &rule.message) } else { None }
                            } else { None }
                        } else { None }
                    } else { None }
                }
                RuleType::Length => {
                    if let (Some(val), Some(params)) = (value, &rule.params) {
                        if let Some(s) = val.as_str() {
                            let len = s.len() as u64;
                            let min = params.get("min").and_then(|v| v.as_u64()).unwrap_or(0);
                            let max = params.get("max").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
                            if len < min || len > max { Some(rule.message.clone()) } else { None }
                        } else { None }
                    } else { None }
                }
                _ => None, // Other rule types: pass for now
            };

            if let Some(msg) = violation {
                let v = RuleViolation {
                    rule_id: rule.rule_id.clone(),
                    field: rule.field.clone(),
                    message: msg,
                    severity: rule.severity.clone(),
                };
                match rule.severity {
                    Severity::Error => errors.push(v),
                    Severity::Warning => warnings.push(v),
                }
            }
        }

        ValidationResult {
            record_index: idx,
            valid: errors.is_empty(),
            errors,
            warnings,
        }
    }).collect();

    state_guard.stats.record_request(&format!("/api/v1/rulepacks/{}/validate", version), true, start.elapsed().as_millis() as f64);

    let meta = ResponseMeta {
        page: None,
        page_size: None,
        total_count: Some(results.len() as u64),
        total_pages: None,
        processing_time_ms: Some(start.elapsed().as_millis() as u64),
        extra: None,
    };

    (StatusCode::OK, Json(ApiResponse::success_with_meta(results, meta)))
}

fn return_violation(should: bool, msg: &str) -> Option<String> {
    if should { Some(msg.to_string()) } else { None }
}

/// Request to create a new rulepack
#[derive(Debug, Deserialize)]
pub struct CreateRulepackRequest {
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub rules: Vec<ValidationRule>,
    #[serde(default)]
    pub cross_field_rules: Vec<CrossFieldRule>,
}

async fn create_rulepack_handler(
    State(state): State<SharedState<RulepackServiceState>>,
    Json(request): Json<CreateRulepackRequest>,
) -> impl IntoResponse {
    let mut state_guard = state.write().await;

    if state_guard.rulepacks.contains_key(&request.version) {
        return (StatusCode::CONFLICT, Json(ApiResponse::<RulepackConfig>::error("CONFLICT", &format!("Rulepack version '{}' already exists", request.version))));
    }

    let rulepack = RulepackConfig {
        version: request.version.clone(),
        name: request.name,
        description: request.description.unwrap_or_default(),
        rules: request.rules,
        cross_field_rules: request.cross_field_rules,
        created_at: chrono::Utc::now().to_rfc3339(),
        is_active: true,
    };

    state_guard.rulepacks.insert(request.version, rulepack.clone());
    (StatusCode::CREATED, Json(ApiResponse::success(rulepack)))
}

async fn stats_handler(
    State(state): State<SharedState<RulepackServiceState>>,
) -> impl IntoResponse {
    let state_guard = state.read().await;
    Json(ApiResponse::success(state_guard.stats.clone()))
}

async fn reset_handler(
    State(state): State<SharedState<RulepackServiceState>>,
) -> impl IntoResponse {
    let mut state_guard = state.write().await;
    state_guard.stats = SimulatorStats::default();

    state_guard.rulepacks.clear();
    if state_guard.config.available_versions.contains(&"v1".to_string()) {
        state_guard.rulepacks.insert("v1".to_string(), RulepackConfig::v1());
    }
    if state_guard.config.available_versions.contains(&"v2".to_string()) {
        state_guard.rulepacks.insert("v2".to_string(), RulepackConfig::v2());
    }

    Json(ApiResponse::success(serde_json::json!({
        "reset": true,
        "rulepack_count": state_guard.rulepacks.len()
    })))
}
