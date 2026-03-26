//! # Mapping Service Simulator
//!
//! Simulates a Mapping Service that provides versioned field mapping configurations
//! for transforming source data to SLIK format.
//!
//! ## Endpoints
//!
//! - `GET /health` - Health check
//! - `GET /api/v1/mappings` - List available mapping versions
//! - `GET /api/v1/mappings/:version` - Get mapping configuration by version
//! - `GET /api/v1/mappings/:version/fields` - Get field mappings for a version
//! - `GET /api/v1/mappings/:version/validate` - Validate a mapping version exists
//! - `POST /api/v1/mappings` - Create a new mapping version (for testing)
//! - `GET /api/v1/stats` - Get simulator statistics
//! - `POST /api/v1/reset` - Reset statistics

//! # Mapping Service Simulator
//!
//! Simulates the service responsible for mapping source data (e.g. Core Banking)
//! to the target schema (e.g. SLIK/OJK format).
//!
//! ## Business Purpose
//!
//! The Mapping Service creates an abstraction layer between internal data structures
//! and external regulatory reporting requirements. It allows the system to:
//! 1. Transform data fields (rename, format, calculate).
//! 2. Apply default values for missing data.
//! 3. Validate data integrity before processing.
//! 4. Handle multiple versions of mapping configurations (e.g., for different regulation periods).
//!
//! ## Key Features
//!
//! - **Versioning**: Supports multiple active mapping versions (v1, v2, v3).
//! - **Transformation Types**:
//!   - `Direct`: Copy value as-is.
//!   - `Computed`: Calculate value using an expression.
//!   - `Concat`: Combine multiple fields.
//!   - `Lookup`: Map values using a reference table.
//!   - `Constant`: Use a fixed value.
//! - **Validation**:
//!   - Required fields.
//!   - Length constraints.
//!   - Pattern matching (Regex).
//!   - Range checks (Min/Max).
//!
//! ## Input/Output
//!
//! - **Input**: Source JSON record (e.g., CreditRecord from Core Banking).
//! - **Output**: Mapped/Transformed JSON record ready for Rulepack validation.
//!
//! ## API Endpoints
//!
//! - `GET /api/v1/mappings` - List all available mapping configurations.
//! - `GET /api/v1/mappings/:version` - Get a specific mapping configuration.
//! - `GET /api/v1/mappings/:version/fields` - Get just the field definitions.
//! - `POST /api/v1/mappings/:version/validate` - Validate a record against the mapping schema.

use crate::{
    config::{FailureType, MappingServiceConfig},
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

/// Mapping configuration for a specific version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingConfig {
    /// Version identifier (e.g., "v1", "v2")
    pub version: String,
    /// Display name
    pub name: String,
    /// Description
    pub description: String,
    /// Target schema version
    pub target_schema: String,
    /// Field mappings
    pub field_mappings: Vec<FieldMapping>,
    /// Transformation rules
    pub transformations: Vec<TransformationRule>,
    /// Validation rules for the mapping
    pub validation_rules: Vec<MappingValidation>,
    /// Creation timestamp
    pub created_at: String,
    /// Last update timestamp
    pub updated_at: String,
    /// Whether this mapping is active
    pub is_active: bool,
    /// Metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl MappingConfig {
    /// Create v1 mapping configuration (basic mapping)
    pub fn v1() -> Self {
        Self {
            version: "v1".to_string(),
            name: "SLIK Basic Mapping v1".to_string(),
            description: "Basic field mapping for SLIK credit reporting".to_string(),
            target_schema: "SLIK-2023".to_string(),
            field_mappings: vec![
                FieldMapping::direct("id", "credit_id"),
                FieldMapping::direct("nik", "debtor_nik"),
                FieldMapping::direct("nama_lengkap", "debtor_name"),
                FieldMapping::direct("jenis_fasilitas", "facility_type"),
                FieldMapping::direct("jumlah_kredit", "credit_amount"),
                FieldMapping::direct("mata_uang", "currency"),
                FieldMapping::direct("suku_bunga", "interest_rate"),
                FieldMapping::direct("tanggal_mulai", "start_date"),
                FieldMapping::direct("tanggal_jatuh_tempo", "maturity_date"),
                FieldMapping::direct("saldo_outstanding", "outstanding_balance"),
                FieldMapping::direct("kolektabilitas", "collectability"),
                FieldMapping::direct("kode_cabang", "branch_code"),
                FieldMapping::direct("account_officer", "officer_id"),
            ],
            transformations: vec![
                TransformationRule::new("uppercase", "debtor_name", "UPPER(value)"),
                TransformationRule::new("date_format", "start_date", "FORMAT(value, 'YYYYMMDD')"),
                TransformationRule::new("date_format", "maturity_date", "FORMAT(value, 'YYYYMMDD')"),
            ],
            validation_rules: vec![
                MappingValidation::required("debtor_nik"),
                MappingValidation::required("debtor_name"),
                MappingValidation::required("credit_amount"),
            ],
            created_at: "2023-01-15T00:00:00Z".to_string(),
            updated_at: "2023-06-20T00:00:00Z".to_string(),
            is_active: true,
            metadata: None,
        }
    }

    /// Create v2 mapping configuration (enhanced with additional fields)
    pub fn v2() -> Self {
        let mut mapping = Self::v1();
        mapping.version = "v2".to_string();
        mapping.name = "SLIK Enhanced Mapping v2".to_string();
        mapping.description = "Enhanced field mapping with additional fields and transformations".to_string();
        mapping.target_schema = "SLIK-2024".to_string();

        // Add additional field mappings
        mapping.field_mappings.extend(vec![
            FieldMapping::direct("last_updated", "last_update_timestamp"),
            FieldMapping::computed("risk_category", "CASE WHEN kolektabilitas <= 2 THEN 'LOW' WHEN kolektabilitas <= 3 THEN 'MEDIUM' ELSE 'HIGH' END"),
            FieldMapping::computed("is_performing", "kolektabilitas <= 2"),
            FieldMapping::with_default("reporting_period", "period", "CURRENT_MONTH"),
        ]);

        // Add additional transformations
        mapping.transformations.extend(vec![
            TransformationRule::new("trim", "debtor_nik", "TRIM(value)"),
            TransformationRule::new("numeric_format", "credit_amount", "FORMAT(value, '0.00')"),
            TransformationRule::new("numeric_format", "outstanding_balance", "FORMAT(value, '0.00')"),
        ]);

        // Add additional validations
        mapping.validation_rules.extend(vec![
            MappingValidation::required("facility_type"),
            MappingValidation::required("currency"),
            MappingValidation::range("collectability", 1, 5),
        ]);

        mapping.created_at = "2024-01-10T00:00:00Z".to_string();
        mapping.updated_at = "2024-03-15T00:00:00Z".to_string();

        mapping
    }

    /// Create v3 mapping configuration (with regulatory compliance fields)
    pub fn v3() -> Self {
        let mut mapping = Self::v2();
        mapping.version = "v3".to_string();
        mapping.name = "SLIK Regulatory Mapping v3".to_string();
        mapping.description = "Regulatory compliance mapping with OJK required fields".to_string();
        mapping.target_schema = "SLIK-2024-REG".to_string();

        // Add regulatory compliance fields
        mapping.field_mappings.extend(vec![
            FieldMapping::computed("reporting_bank_code", "'BANKXYZ'"),
            FieldMapping::computed("reporting_timestamp", "NOW()"),
            FieldMapping::computed("record_hash", "SHA256(CONCAT(credit_id, debtor_nik, credit_amount))"),
            FieldMapping::with_default("restructured_flag", "is_restructured", "N"),
            FieldMapping::with_default("write_off_flag", "is_write_off", "N"),
        ]);

        // Add regulatory transformations
        mapping.transformations.extend(vec![
            TransformationRule::new("pad_left", "debtor_nik", "LPAD(value, 16, '0')"),
            TransformationRule::new("currency_normalize", "currency", "IF(value='IDR', 'IDR', 'FCY')"),
        ]);

        // Add regulatory validations
        mapping.validation_rules.extend(vec![
            MappingValidation::length("debtor_nik", 16, 16),
            MappingValidation::pattern("branch_code", r"^[A-Z]{3}\d{3}$"),
        ]);

        mapping.created_at = "2024-06-01T00:00:00Z".to_string();
        mapping.updated_at = "2024-06-01T00:00:00Z".to_string();

        mapping
    }
}

/// Individual field mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldMapping {
    /// Source field name
    pub source_field: String,
    /// Target field name
    pub target_field: String,
    /// Mapping type
    pub mapping_type: FieldMappingType,
    /// Expression for computed fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    /// Default value if source is null/missing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    /// Whether this field is required
    pub required: bool,
    /// Field description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl FieldMapping {
    /// Create a direct field mapping (1:1)
    pub fn direct(source: &str, target: &str) -> Self {
        Self {
            source_field: source.to_string(),
            target_field: target.to_string(),
            mapping_type: FieldMappingType::Direct,
            expression: None,
            default_value: None,
            required: false,
            description: None,
        }
    }

    /// Create a computed field mapping
    pub fn computed(target: &str, expression: &str) -> Self {
        Self {
            source_field: "".to_string(),
            target_field: target.to_string(),
            mapping_type: FieldMappingType::Computed,
            expression: Some(expression.to_string()),
            default_value: None,
            required: false,
            description: None,
        }
    }

    /// Create a field mapping with default value
    pub fn with_default(source: &str, target: &str, default: &str) -> Self {
        Self {
            source_field: source.to_string(),
            target_field: target.to_string(),
            mapping_type: FieldMappingType::Direct,
            expression: None,
            default_value: Some(default.to_string()),
            required: false,
            description: None,
        }
    }

    /// Set the field as required
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Set the description
    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }
}

/// Type of field mapping
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldMappingType {
    /// Direct 1:1 field mapping
    Direct,
    /// Computed field from expression
    Computed,
    /// Concatenation of multiple fields
    Concat,
    /// Lookup from reference table
    Lookup,
    /// Constant value
    Constant,
}

/// Transformation rule to apply to a field
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformationRule {
    /// Rule name
    pub name: String,
    /// Target field to transform
    pub target_field: String,
    /// Transformation expression
    pub expression: String,
    /// Order of execution
    pub order: u32,
    /// Whether this transformation is enabled
    pub enabled: bool,
}

impl TransformationRule {
    pub fn new(name: &str, target_field: &str, expression: &str) -> Self {
        Self {
            name: name.to_string(),
            target_field: target_field.to_string(),
            expression: expression.to_string(),
            order: 0,
            enabled: true,
        }
    }

    pub fn with_order(mut self, order: u32) -> Self {
        self.order = order;
        self
    }
}

/// Validation rule for mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingValidation {
    /// Validation type
    pub validation_type: ValidationType,
    /// Target field
    pub field: String,
    /// Validation parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, serde_json::Value>>,
    /// Error message if validation fails
    pub error_message: String,
}

impl MappingValidation {
    pub fn required(field: &str) -> Self {
        Self {
            validation_type: ValidationType::Required,
            field: field.to_string(),
            params: None,
            error_message: format!("Field '{}' is required", field),
        }
    }

    pub fn length(field: &str, min: u32, max: u32) -> Self {
        let mut params = HashMap::new();
        params.insert("min".to_string(), serde_json::json!(min));
        params.insert("max".to_string(), serde_json::json!(max));

        Self {
            validation_type: ValidationType::Length,
            field: field.to_string(),
            params: Some(params),
            error_message: format!("Field '{}' must be between {} and {} characters", field, min, max),
        }
    }

    pub fn range(field: &str, min: i64, max: i64) -> Self {
        let mut params = HashMap::new();
        params.insert("min".to_string(), serde_json::json!(min));
        params.insert("max".to_string(), serde_json::json!(max));

        Self {
            validation_type: ValidationType::Range,
            field: field.to_string(),
            params: Some(params),
            error_message: format!("Field '{}' must be between {} and {}", field, min, max),
        }
    }

    pub fn pattern(field: &str, pattern: &str) -> Self {
        let mut params = HashMap::new();
        params.insert("pattern".to_string(), serde_json::json!(pattern));

        Self {
            validation_type: ValidationType::Pattern,
            field: field.to_string(),
            params: Some(params),
            error_message: format!("Field '{}' must match pattern '{}'", field, pattern),
        }
    }
}

/// Type of validation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationType {
    Required,
    Length,
    Range,
    Pattern,
    Custom,
}

/// Query parameters for listing mappings
#[derive(Debug, Deserialize)]
pub struct ListMappingsParams {
    pub active_only: Option<bool>,
}

/// Request to create a new mapping
#[derive(Debug, Deserialize)]
pub struct CreateMappingRequest {
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub field_mappings: Vec<FieldMapping>,
}

// ============================================================================
// Simulator State
// ============================================================================

/// Internal state for Mapping Service Simulator
pub struct MappingServiceState {
    pub config: MappingServiceConfig,
    pub mappings: HashMap<String, MappingConfig>,
    pub stats: SimulatorStats,
    pub started_at: Instant,
    pub ready: bool,
}

impl MappingServiceState {
    pub fn new(config: MappingServiceConfig) -> Self {
        let mut mappings = HashMap::new();

        // Pre-populate with default versions
        if config.available_versions.contains(&"v1".to_string()) {
            mappings.insert("v1".to_string(), MappingConfig::v1());
        }
        if config.available_versions.contains(&"v2".to_string()) {
            mappings.insert("v2".to_string(), MappingConfig::v2());
        }
        if config.available_versions.contains(&"v3".to_string()) {
            mappings.insert("v3".to_string(), MappingConfig::v3());
        }

        Self {
            config,
            mappings,
            stats: SimulatorStats::default(),
            started_at: Instant::now(),
            ready: false,
        }
    }
}

// ============================================================================
// Mapping Service Simulator
// ============================================================================

/// Mapping Service Simulator implementation
pub struct MappingServiceSimulator {
    state: SharedState<MappingServiceState>,
    config: MappingServiceConfig,
}

impl MappingServiceSimulator {
    /// Create a new Mapping Service Simulator
    pub fn new(config: MappingServiceConfig) -> Self {
        let state = shared_state(MappingServiceState::new(config.clone()));
        Self { state, config }
    }

    /// Run the simulator HTTP server
    pub async fn run(&self, shutdown_rx: oneshot::Receiver<()>) -> SimulatorResult<()> {
        // Mark as ready
        {
            let mut state = self.state.write().await;
            state.ready = true;
        }

        let app = self.create_router();
        let addr: std::net::SocketAddr = self.config.socket_addr().parse()
            .map_err(|e| SimulatorError::ConfigError(format!("Invalid address: {}", e)))?;

        tracing::info!("Mapping Service Simulator listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr).await
            .map_err(|e| SimulatorError::BindError(e.to_string()))?;

        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
                tracing::info!("Mapping Service Simulator shutting down");
            })
            .await
            .map_err(|e| SimulatorError::StartError(e.to_string()))?;

        Ok(())
    }

    /// Create the router with all endpoints
    fn create_router(&self) -> Router {
        let state = self.state.clone();

        Router::new()
            .route("/health", get(health_handler))
            .route("/api/v1/mappings", get(list_mappings_handler))
            .route("/api/v1/mappings", post(create_mapping_handler))
            .route("/api/v1/mappings/:version", get(get_mapping_handler))
            .route("/api/v1/mappings/:version/fields", get(get_fields_handler))
            .route("/api/v1/mappings/:version/validate", get(validate_mapping_handler))
            .route("/api/v1/stats", get(stats_handler))
            .route("/api/v1/reset", post(reset_handler))
            .with_state(state)
    }

    /// Get a specific mapping version
    pub async fn get_mapping(&self, version: &str) -> Option<MappingConfig> {
        self.state.read().await.mappings.get(version).cloned()
    }

    /// Add a custom mapping (for testing)
    pub async fn add_mapping(&self, mapping: MappingConfig) {
        self.state.write().await.mappings.insert(mapping.version.clone(), mapping);
    }
}

#[async_trait::async_trait]
impl Simulator for MappingServiceSimulator {
    fn name(&self) -> &str {
        "mapping-service"
    }

    fn port(&self) -> u16 {
        self.config.port
    }

    async fn health(&self) -> HealthStatus {
        let state = self.state.read().await;
        let uptime = state.started_at.elapsed().as_secs();

        if state.ready {
            HealthStatus::healthy(self.name(), "1.0.0", uptime)
                .with_details("mapping_count", serde_json::json!(state.mappings.len()))
                .with_details("available_versions", serde_json::json!(
                    state.mappings.keys().collect::<Vec<_>>()
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

/// Health check endpoint
async fn health_handler(
    State(state): State<SharedState<MappingServiceState>>,
) -> impl IntoResponse {
    let state = state.read().await;
    let uptime = state.started_at.elapsed().as_secs();

    if state.ready {
        let health = HealthStatus::healthy("mapping-service", "1.0.0", uptime)
            .with_details("mapping_count", serde_json::json!(state.mappings.len()));
        (StatusCode::OK, Json(health))
    } else {
        let health = HealthStatus::unhealthy("mapping-service", "Not ready");
        (StatusCode::SERVICE_UNAVAILABLE, Json(health))
    }
}

/// List available mappings
async fn list_mappings_handler(
    State(state): State<SharedState<MappingServiceState>>,
    Query(params): Query<ListMappingsParams>,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut state_guard = state.write().await;

    // Check for failure injection
    let failure = state_guard.config.failure_injection.random_failure().cloned();
    if let Some(ref failure) = failure {
        state_guard.stats.record_request("/api/v1/mappings", false, start.elapsed().as_millis() as f64);
        return match failure {
            FailureType::InternalError => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<MappingSummary>>::error("ERR500", "Internal server error")))
            }
            FailureType::ServiceUnavailable => {
                (StatusCode::SERVICE_UNAVAILABLE, Json(ApiResponse::<Vec<MappingSummary>>::error("ERR503", "Service unavailable")))
            }
            _ => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<Vec<MappingSummary>>::error("ERR500", "Internal server error")))
            }
        };
    }

    // Apply latency
    state_guard.config.latency.apply().await;

    let active_only = params.active_only.unwrap_or(false);

    let summaries: Vec<MappingSummary> = state_guard.mappings
        .values()
        .filter(|m| !active_only || m.is_active)
        .map(|m| MappingSummary {
            version: m.version.clone(),
            name: m.name.clone(),
            target_schema: m.target_schema.clone(),
            field_count: m.field_mappings.len(),
            is_active: m.is_active,
            updated_at: m.updated_at.clone(),
        })
        .collect();

    state_guard.stats.record_request("/api/v1/mappings", true, start.elapsed().as_millis() as f64);

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

/// Get mapping by version
async fn get_mapping_handler(
    State(state): State<SharedState<MappingServiceState>>,
    Path(version): Path<String>,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut state_guard = state.write().await;

    // Check for failure injection
    let failure = state_guard.config.failure_injection.random_failure().cloned();
    if let Some(ref failure) = failure {
        state_guard.stats.record_request(&format!("/api/v1/mappings/{}", version), false, start.elapsed().as_millis() as f64);
        return match failure {
            FailureType::InternalError => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<MappingConfig>::error("ERR500", "Internal server error")))
            }
            _ => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<MappingConfig>::error("ERR500", "Internal server error")))
            }
        };
    }

    // Apply latency
    state_guard.config.latency.apply().await;

    if let Some(mapping) = state_guard.mappings.get(&version).cloned() {
        state_guard.stats.record_request(&format!("/api/v1/mappings/{}", version), true, start.elapsed().as_millis() as f64);
        (StatusCode::OK, Json(ApiResponse::success(mapping)))
    } else {
        state_guard.stats.record_request(&format!("/api/v1/mappings/{}", version), false, start.elapsed().as_millis() as f64);
        (StatusCode::NOT_FOUND, Json(ApiResponse::<MappingConfig>::error("NOT_FOUND", &format!("Mapping version '{}' not found", version))))
    }
}

/// Get field mappings for a version
async fn get_fields_handler(
    State(state): State<SharedState<MappingServiceState>>,
    Path(version): Path<String>,
) -> impl IntoResponse {
    let state_guard = state.read().await;

    if let Some(mapping) = state_guard.mappings.get(&version) {
        (StatusCode::OK, Json(ApiResponse::success(mapping.field_mappings.clone())))
    } else {
        (StatusCode::NOT_FOUND, Json(ApiResponse::<Vec<FieldMapping>>::error("NOT_FOUND", &format!("Mapping version '{}' not found", version))))
    }
}

/// Validate a mapping version exists
async fn validate_mapping_handler(
    State(state): State<SharedState<MappingServiceState>>,
    Path(version): Path<String>,
) -> impl IntoResponse {
    let state_guard = state.read().await;

    let exists = state_guard.mappings.contains_key(&version);
    let is_active = state_guard.mappings.get(&version).map(|m| m.is_active).unwrap_or(false);

    Json(ApiResponse::success(serde_json::json!({
        "version": version,
        "exists": exists,
        "is_active": is_active
    })))
}

/// Create a new mapping (for testing)
async fn create_mapping_handler(
    State(state): State<SharedState<MappingServiceState>>,
    Json(request): Json<CreateMappingRequest>,
) -> impl IntoResponse {
    let mut state_guard = state.write().await;

    if state_guard.mappings.contains_key(&request.version) {
        return (StatusCode::CONFLICT, Json(ApiResponse::<MappingConfig>::error("CONFLICT", &format!("Mapping version '{}' already exists", request.version))));
    }

    let mapping = MappingConfig {
        version: request.version.clone(),
        name: request.name,
        description: request.description.unwrap_or_default(),
        target_schema: "SLIK-CUSTOM".to_string(),
        field_mappings: request.field_mappings,
        transformations: Vec::new(),
        validation_rules: Vec::new(),
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        is_active: true,
        metadata: None,
    };

    state_guard.mappings.insert(request.version, mapping.clone());

    (StatusCode::CREATED, Json(ApiResponse::success(mapping)))
}

/// Get simulator statistics
async fn stats_handler(
    State(state): State<SharedState<MappingServiceState>>,
) -> impl IntoResponse {
    let state_guard = state.read().await;
    Json(ApiResponse::success(state_guard.stats.clone()))
}

/// Reset simulator state
async fn reset_handler(
    State(state): State<SharedState<MappingServiceState>>,
) -> impl IntoResponse {
    let mut state_guard = state.write().await;
    state_guard.stats = SimulatorStats::default();

    // Re-initialize default mappings
    state_guard.mappings.clear();
    if state_guard.config.available_versions.contains(&"v1".to_string()) {
        state_guard.mappings.insert("v1".to_string(), MappingConfig::v1());
    }
    if state_guard.config.available_versions.contains(&"v2".to_string()) {
        state_guard.mappings.insert("v2".to_string(), MappingConfig::v2());
    }
    if state_guard.config.available_versions.contains(&"v3".to_string()) {
        state_guard.mappings.insert("v3".to_string(), MappingConfig::v3());
    }

    Json(ApiResponse::success(serde_json::json!({
        "reset": true,
        "mapping_count": state_guard.mappings.len()
    })))
}

/// Summary of a mapping for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingSummary {
    pub version: String,
    pub name: String,
    pub target_schema: String,
    pub field_count: usize,
    pub is_active: bool,
    pub updated_at: String,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mapping_config_v1() {
        let mapping = MappingConfig::v1();
        assert_eq!(mapping.version, "v1");
        assert_eq!(mapping.target_schema, "SLIK-2023");
        assert!(!mapping.field_mappings.is_empty());
        assert!(mapping.is_active);
    }

    #[test]
    fn test_mapping_config_v2() {
        let mapping = MappingConfig::v2();
        assert_eq!(mapping.version, "v2");
        assert_eq!(mapping.target_schema, "SLIK-2024");
        // v2 should have more fields than v1
        assert!(mapping.field_mappings.len() > MappingConfig::v1().field_mappings.len());
    }

    #[test]
    fn test_mapping_config_v3() {
        let mapping = MappingConfig::v3();
        assert_eq!(mapping.version, "v3");
        assert_eq!(mapping.target_schema, "SLIK-2024-REG");
        // v3 should have regulatory fields
        assert!(mapping.field_mappings.iter().any(|f| f.target_field == "record_hash"));
    }

    #[test]
    fn test_field_mapping_direct() {
        let mapping = FieldMapping::direct("source", "target");
        assert_eq!(mapping.source_field, "source");
        assert_eq!(mapping.target_field, "target");
        assert_eq!(mapping.mapping_type, FieldMappingType::Direct);
        assert!(mapping.expression.is_none());
    }

    #[test]
    fn test_field_mapping_computed() {
        let mapping = FieldMapping::computed("target", "UPPER(value)");
        assert_eq!(mapping.target_field, "target");
        assert_eq!(mapping.mapping_type, FieldMappingType::Computed);
        assert_eq!(mapping.expression, Some("UPPER(value)".to_string()));
    }

    #[test]
    fn test_field_mapping_with_default() {
        let mapping = FieldMapping::with_default("source", "target", "DEFAULT");
        assert_eq!(mapping.default_value, Some("DEFAULT".to_string()));
    }

    #[test]
    fn test_mapping_validation_required() {
        let validation = MappingValidation::required("field_name");
        assert_eq!(validation.validation_type, ValidationType::Required);
        assert_eq!(validation.field, "field_name");
    }

    #[test]
    fn test_mapping_validation_length() {
        let validation = MappingValidation::length("nik", 16, 16);
        assert_eq!(validation.validation_type, ValidationType::Length);
        let params = validation.params.unwrap();
        assert_eq!(params.get("min"), Some(&serde_json::json!(16)));
        assert_eq!(params.get("max"), Some(&serde_json::json!(16)));
    }

    #[test]
    fn test_mapping_validation_range() {
        let validation = MappingValidation::range("collectability", 1, 5);
        assert_eq!(validation.validation_type, ValidationType::Range);
        let params = validation.params.unwrap();
        assert_eq!(params.get("min"), Some(&serde_json::json!(1)));
        assert_eq!(params.get("max"), Some(&serde_json::json!(5)));
    }

    #[test]
    fn test_transformation_rule() {
        let rule = TransformationRule::new("uppercase", "name", "UPPER(value)")
            .with_order(1);
        assert_eq!(rule.name, "uppercase");
        assert_eq!(rule.target_field, "name");
        assert_eq!(rule.order, 1);
        assert!(rule.enabled);
    }

    #[test]
    fn test_state_initialization() {
        let config = MappingServiceConfig::default();
        let state = MappingServiceState::new(config);

        // Should have pre-populated mappings
        assert!(state.mappings.contains_key("v1"));
        assert!(state.mappings.contains_key("v2"));
    }
}
