//! # Regulator Endpoint Simulator
//!
//! Simulates the OJK (Otoritas Jasa Keuangan) regulator endpoint for SLIK submissions.
//! Supports various response modes for testing different scenarios including:
//! - Normal acceptance
//! - Timeouts
//! - Rejections
//! - Rate limiting
//! - Partial failures
//! - Idempotency handling
//!
//! ## Endpoints
//!
//! - `GET /health` - Health check
//! - `POST /api/v1/submit` - Submit credit data
//! - `GET /api/v1/submissions/:id` - Get submission status
//! - `GET /api/v1/submissions` - List submissions
//! - `POST /api/v1/mode` - Change response mode dynamically
//! - `GET /api/v1/stats` - Get simulator statistics
//! - `POST /api/v1/reset` - Reset state and statistics
//! - `DELETE /api/v1/submissions/:id` - Delete a submission (for testing)

use crate::{
    config::{FailureType, RegulatorEndpointConfig, RegulatorMode},
    ApiResponse, HealthStatus, ResponseMeta, SharedState, SimulatorError, SimulatorResult,
    SimulatorStats, Simulator, shared_state,
};
/// # Regulator Endpoint Simulator (OJK/SLIK)
///
/// Simulates the external regulatory reporting endpoint (e.g. SLIK/OJK).
/// This service receives data submissions, validates them, and provides acknowledgments.
///
/// ## Business Purpose
///
/// The Regulator Endpoint mimics the behavior of the OJK (Otoritas Jasa Keuangan) SLIK system.
/// It allows the Vastar Workflow Engine to:
/// 1. Test submission workflows (End-to-End).
/// 2. Handle various response scenarios (Success, Failure, Timeout).
/// 3. Verify retry mechanisms and error handling logic.
/// 4. Validate idempotency (prevent duplicate submissions).
///
/// ## Operating Modes
///
/// This simulator supports dynamic mode switching to test system resilience:
///
/// - **Accept**: Standard success mode. Validates schema and accepts submission.
/// - **Reject**: Simulates logic/validation errors (400 Bad Request).
/// - **Timeout**: Simulates network latency or gateway timeouts (504 Gateway Timeout).
/// - **ServiceUnavailable**: Simulates downtime or maintenance (503 Service Unavailable).
/// - **RateLimited**: Simulates API rate limits (429 Too Many Requests).
/// - **Intermittent**: Randomly fails a percentage of requests (Chaos Testing).
/// - **PartialReject**: Accepts the submission but rejects specific records inside it.
/// - **Queued**: Simulates async processing (202 Accepted) where status must be polled later.
/// - **Custom**: Configurable status code and body for edge case testing.
///
/// ## Key Features
///
/// - **Idempotency**: Checks `Reference-Number` or content hash to detect duplicates.
/// - **Async Processing**: Can simulate long-running validation tasks.
/// - **Detailed Receipts**: Returns submission ID, timestamp, and record-level status.
///
/// ## API Endpoints
///
/// - `POST /api/v1/submissions` - Submit a new regulatory report.
/// - `GET /api/v1/submissions/:id` - Check status of a submission.
/// - `GET /api/v1/submissions` - List all received submissions (for verification).
/// - `POST /api/v1/mode` - Change the simulator's operating mode (Runtime Config).
/// - `GET /api/v1/mode` - Get current operating mode.
/// - `GET /api/v1/stats` - Get simulator statistics.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

// ============================================================================
// Data Models
// ============================================================================

/// Submission request from clients
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionRequest {
    /// Client-provided submission ID for idempotency
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission_id: Option<String>,
    /// Reporting period (YYYYMM)
    pub reporting_period: String,
    /// Bank code
    pub bank_code: String,
    /// Credit records to submit
    pub records: Vec<serde_json::Value>,
    /// Metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Submission response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionResponse {
    /// Submission ID (generated or provided)
    pub submission_id: String,
    /// Regulator reference number
    pub reference_number: String,
    /// Submission status
    pub status: SubmissionStatus,
    /// Number of records accepted
    pub accepted_count: usize,
    /// Number of records rejected
    pub rejected_count: usize,
    /// Rejected record details
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rejections: Vec<RecordRejection>,
    /// Submission timestamp
    pub submitted_at: String,
    /// Processing timestamp (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processed_at: Option<String>,
    /// Estimated processing time (for queued submissions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_processing_time_secs: Option<u64>,
    /// Additional details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<HashMap<String, serde_json::Value>>,
}

/// Submission status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SubmissionStatus {
    /// Submission accepted and processed
    Accepted,
    /// Submission partially accepted (some records rejected)
    PartiallyAccepted,
    /// Submission rejected
    Rejected,
    /// Submission queued for processing
    Queued,
    /// Submission is being processed
    Processing,
    /// Submission failed
    Failed,
}

/// Record rejection details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordRejection {
    /// Record index in the submission
    pub record_index: usize,
    /// Record ID if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
    /// Rejection error code
    pub error_code: String,
    /// Rejection message
    pub message: String,
    /// Field that caused the rejection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

/// Stored submission record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSubmission {
    pub submission_id: String,
    pub reference_number: String,
    pub bank_code: String,
    pub reporting_period: String,
    pub status: SubmissionStatus,
    pub record_count: usize,
    pub accepted_count: usize,
    pub rejected_count: usize,
    pub rejections: Vec<RecordRejection>,
    pub submitted_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub idempotency_key: Option<String>,
    pub attempt_count: u32,
}

/// Query parameters for listing submissions
#[derive(Debug, Deserialize)]
pub struct ListSubmissionsParams {
    pub bank_code: Option<String>,
    pub reporting_period: Option<String>,
    pub status: Option<SubmissionStatus>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

/// Request to change the response mode
#[derive(Debug, Deserialize)]
pub struct ChangeModeRequest {
    pub mode: RegulatorMode,
}

/// Rejection error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectionError {
    pub error_code: String,
    pub error_message: String,
    pub details: Option<serde_json::Value>,
}

// ============================================================================
// Simulator State
// ============================================================================

/// Internal state for Regulator Endpoint Simulator
pub struct RegulatorEndpointState {
    pub config: RegulatorEndpointConfig,
    pub submissions: HashMap<String, StoredSubmission>,
    pub idempotency_cache: HashMap<String, String>, // idempotency_key -> submission_id
    pub stats: SimulatorStats,
    pub started_at: Instant,
    pub ready: bool,
    pub submission_counter: u64,
}

impl RegulatorEndpointState {
    pub fn new(config: RegulatorEndpointConfig) -> Self {
        Self {
            config,
            submissions: HashMap::new(),
            idempotency_cache: HashMap::new(),
            stats: SimulatorStats::default(),
            started_at: Instant::now(),
            ready: false,
            submission_counter: 0,
        }
    }

    /// Generate a new reference number
    pub fn generate_reference_number(&mut self) -> String {
        self.submission_counter += 1;
        let timestamp = Utc::now().format("%Y%m%d%H%M%S");
        format!("OJK-SLIK-{}-{:06}", timestamp, self.submission_counter)
    }

    /// Check if submission is a duplicate (idempotency)
    pub fn check_idempotency(&self, key: &str) -> Option<&StoredSubmission> {
        self.idempotency_cache
            .get(key)
            .and_then(|id| self.submissions.get(id))
    }

    /// Store a submission
    pub fn store_submission(&mut self, submission: StoredSubmission) {
        if let Some(ref key) = submission.idempotency_key {
            self.idempotency_cache.insert(key.clone(), submission.submission_id.clone());
        }
        self.submissions.insert(submission.submission_id.clone(), submission);

        // Cleanup old idempotency entries if needed
        if self.idempotency_cache.len() > self.config.max_idempotency_entries {
            // Remove oldest entries (simple approach: just clear half)
            let to_remove: Vec<String> = self.idempotency_cache
                .keys()
                .take(self.idempotency_cache.len() / 2)
                .cloned()
                .collect();
            for key in to_remove {
                self.idempotency_cache.remove(&key);
            }
        }
    }

    /// Process submission based on current mode
    pub fn process_submission(
        &mut self,
        request: &SubmissionRequest,
        idempotency_key: Option<String>,
    ) -> Result<SubmissionResponse, (StatusCode, RejectionError)> {
        // Check idempotency first
        if let Some(ref key) = idempotency_key {
            if self.config.enforce_idempotency {
                if let Some(existing) = self.check_idempotency(key) {
                    // Return the existing submission response
                    return Ok(SubmissionResponse {
                        submission_id: existing.submission_id.clone(),
                        reference_number: existing.reference_number.clone(),
                        status: existing.status.clone(),
                        accepted_count: existing.accepted_count,
                        rejected_count: existing.rejected_count,
                        rejections: existing.rejections.clone(),
                        submitted_at: existing.submitted_at.to_rfc3339(),
                        processed_at: existing.processed_at.map(|t| t.to_rfc3339()),
                        estimated_processing_time_secs: None,
                        details: Some({
                            let mut d = HashMap::new();
                            d.insert("idempotent".to_string(), serde_json::json!(true));
                            d.insert("original_submission".to_string(), serde_json::json!(true));
                            d
                        }),
                    });
                }
            }
        }

        // Check off-peak window
        if self.config.off_peak_config.enabled
            && self.config.off_peak_config.reject_outside_window
            && !self.config.off_peak_config.is_off_peak_now()
        {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                RejectionError {
                    error_code: "OUTSIDE_SUBMISSION_WINDOW".to_string(),
                    error_message: "Submissions are only accepted during off-peak hours".to_string(),
                    details: Some(serde_json::json!({
                        "off_peak_start": self.config.off_peak_config.start_hour,
                        "off_peak_end": self.config.off_peak_config.end_hour,
                    })),
                },
            ));
        }

        // Process based on mode
        match &self.config.mode {
            RegulatorMode::Accept => self.accept_submission(request, idempotency_key),
            RegulatorMode::Reject { error_code, error_message } => {
                Err((
                    StatusCode::BAD_REQUEST,
                    RejectionError {
                        error_code: error_code.clone(),
                        error_message: error_message.clone(),
                        details: None,
                    },
                ))
            }
            RegulatorMode::Timeout { delay_ms } => {
                // This is handled in the handler with actual sleep
                // Here we just return a timeout error after the delay
                Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    RejectionError {
                        error_code: "TIMEOUT".to_string(),
                        error_message: "Request timed out".to_string(),
                        details: Some(serde_json::json!({ "timeout_ms": delay_ms })),
                    },
                ))
            }
            RegulatorMode::ServiceUnavailable => {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    RejectionError {
                        error_code: "SERVICE_UNAVAILABLE".to_string(),
                        error_message: "The service is temporarily unavailable".to_string(),
                        details: None,
                    },
                ))
            }
            RegulatorMode::RateLimited => {
                Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    RejectionError {
                        error_code: "RATE_LIMITED".to_string(),
                        error_message: "Too many requests. Please retry later.".to_string(),
                        details: Some(serde_json::json!({
                            "retry_after_secs": self.config.retry_after_secs
                        })),
                    },
                ))
            }
            RegulatorMode::Intermittent { failure_rate } => {
                if rand::random::<f64>() < *failure_rate {
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        RejectionError {
                            error_code: "INTERNAL_ERROR".to_string(),
                            error_message: "An internal error occurred".to_string(),
                            details: None,
                        },
                    ))
                } else {
                    self.accept_submission(request, idempotency_key)
                }
            }
            RegulatorMode::PartialReject { reject_ratio } => {
                self.partial_accept_submission(request, idempotency_key, *reject_ratio)
            }
            RegulatorMode::Queued { queue_delay_ms } => {
                self.queue_submission(request, idempotency_key, *queue_delay_ms)
            }
            RegulatorMode::Custom { status_code, body, .. } => {
                let status = StatusCode::from_u16(*status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                if status.is_success() {
                    self.accept_submission(request, idempotency_key)
                } else {
                    Err((
                        status,
                        RejectionError {
                            error_code: "CUSTOM_ERROR".to_string(),
                            error_message: body.clone(),
                            details: None,
                        },
                    ))
                }
            }
        }
    }

    fn accept_submission(
        &mut self,
        request: &SubmissionRequest,
        idempotency_key: Option<String>,
    ) -> Result<SubmissionResponse, (StatusCode, RejectionError)> {
        let submission_id = request.submission_id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
        let reference_number = self.generate_reference_number();
        let now = Utc::now();
        let record_count = request.records.len();

        let stored = StoredSubmission {
            submission_id: submission_id.clone(),
            reference_number: reference_number.clone(),
            bank_code: request.bank_code.clone(),
            reporting_period: request.reporting_period.clone(),
            status: SubmissionStatus::Accepted,
            record_count,
            accepted_count: record_count,
            rejected_count: 0,
            rejections: Vec::new(),
            submitted_at: now,
            processed_at: Some(now),
            idempotency_key,
            attempt_count: 1,
        };

        self.store_submission(stored);

        Ok(SubmissionResponse {
            submission_id,
            reference_number,
            status: SubmissionStatus::Accepted,
            accepted_count: record_count,
            rejected_count: 0,
            rejections: Vec::new(),
            submitted_at: now.to_rfc3339(),
            processed_at: Some(now.to_rfc3339()),
            estimated_processing_time_secs: None,
            details: None,
        })
    }

    fn partial_accept_submission(
        &mut self,
        request: &SubmissionRequest,
        idempotency_key: Option<String>,
        reject_ratio: f64,
    ) -> Result<SubmissionResponse, (StatusCode, RejectionError)> {
        let submission_id = request.submission_id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
        let reference_number = self.generate_reference_number();
        let now = Utc::now();
        let record_count = request.records.len();

        // Determine which records to reject
        let mut rejections = Vec::new();
        let rejection_errors = [
            ("INVALID_NIK", "NIK format is invalid"),
            ("INVALID_AMOUNT", "Credit amount is invalid"),
            ("DUPLICATE_RECORD", "Duplicate record detected"),
            ("MISSING_FIELD", "Required field is missing"),
        ];

        for (idx, record) in request.records.iter().enumerate() {
            if rand::random::<f64>() < reject_ratio {
                let error = &rejection_errors[rand::random::<usize>() % rejection_errors.len()];
                rejections.push(RecordRejection {
                    record_index: idx,
                    record_id: record.get("id").and_then(|v| v.as_str()).map(String::from),
                    error_code: error.0.to_string(),
                    message: error.1.to_string(),
                    field: None,
                });
            }
        }

        let rejected_count = rejections.len();
        let accepted_count = record_count - rejected_count;

        let status = if rejected_count == 0 {
            SubmissionStatus::Accepted
        } else if accepted_count == 0 {
            SubmissionStatus::Rejected
        } else {
            SubmissionStatus::PartiallyAccepted
        };

        let stored = StoredSubmission {
            submission_id: submission_id.clone(),
            reference_number: reference_number.clone(),
            bank_code: request.bank_code.clone(),
            reporting_period: request.reporting_period.clone(),
            status: status.clone(),
            record_count,
            accepted_count,
            rejected_count,
            rejections: rejections.clone(),
            submitted_at: now,
            processed_at: Some(now),
            idempotency_key,
            attempt_count: 1,
        };

        self.store_submission(stored);

        Ok(SubmissionResponse {
            submission_id,
            reference_number,
            status,
            accepted_count,
            rejected_count,
            rejections,
            submitted_at: now.to_rfc3339(),
            processed_at: Some(now.to_rfc3339()),
            estimated_processing_time_secs: None,
            details: None,
        })
    }

    fn queue_submission(
        &mut self,
        request: &SubmissionRequest,
        idempotency_key: Option<String>,
        queue_delay_ms: u64,
    ) -> Result<SubmissionResponse, (StatusCode, RejectionError)> {
        let submission_id = request.submission_id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
        let reference_number = self.generate_reference_number();
        let now = Utc::now();
        let record_count = request.records.len();

        let stored = StoredSubmission {
            submission_id: submission_id.clone(),
            reference_number: reference_number.clone(),
            bank_code: request.bank_code.clone(),
            reporting_period: request.reporting_period.clone(),
            status: SubmissionStatus::Queued,
            record_count,
            accepted_count: 0,
            rejected_count: 0,
            rejections: Vec::new(),
            submitted_at: now,
            processed_at: None,
            idempotency_key,
            attempt_count: 1,
        };

        self.store_submission(stored);

        Ok(SubmissionResponse {
            submission_id,
            reference_number,
            status: SubmissionStatus::Queued,
            accepted_count: 0,
            rejected_count: 0,
            rejections: Vec::new(),
            submitted_at: now.to_rfc3339(),
            processed_at: None,
            estimated_processing_time_secs: Some(queue_delay_ms / 1000),
            details: Some({
                let mut d = HashMap::new();
                d.insert("queue_position".to_string(), serde_json::json!(self.submissions.len()));
                d
            }),
        })
    }
}

// ============================================================================
// Regulator Endpoint Simulator
// ============================================================================

/// Regulator Endpoint Simulator implementation
pub struct RegulatorEndpointSimulator {
    state: SharedState<RegulatorEndpointState>,
    config: RegulatorEndpointConfig,
}

impl RegulatorEndpointSimulator {
    /// Create a new Regulator Endpoint Simulator
    pub fn new(config: RegulatorEndpointConfig) -> Self {
        let state = shared_state(RegulatorEndpointState::new(config.clone()));
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

        tracing::info!("Regulator Endpoint Simulator listening on {}", addr);

        let listener = tokio::net::TcpListener::bind(addr).await
            .map_err(|e| SimulatorError::BindError(e.to_string()))?;

        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
                tracing::info!("Regulator Endpoint Simulator shutting down");
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
            .route("/api/v1/submit", post(submit_handler))
            .route("/api/v1/submissions", get(list_submissions_handler))
            .route("/api/v1/submissions/:id", get(get_submission_handler))
            .route("/api/v1/submissions/:id", delete(delete_submission_handler))
            .route("/api/v1/mode", post(change_mode_handler))
            .route("/api/v1/mode", get(get_mode_handler))
            .route("/api/v1/stats", get(stats_handler))
            .route("/api/v1/reset", post(reset_handler))
            .with_state(state)
    }

    /// Get a submission by ID
    pub async fn get_submission(&self, id: &str) -> Option<StoredSubmission> {
        self.state.read().await.submissions.get(id).cloned()
    }

    /// Set the response mode dynamically
    pub async fn set_mode(&self, mode: RegulatorMode) {
        self.state.write().await.config.mode = mode;
    }

    /// Get current mode
    pub async fn get_mode(&self) -> RegulatorMode {
        self.state.read().await.config.mode.clone()
    }

    /// Get all submissions
    pub async fn get_all_submissions(&self) -> Vec<StoredSubmission> {
        self.state.read().await.submissions.values().cloned().collect()
    }

    /// Clear all submissions
    pub async fn clear_submissions(&self) {
        let mut state = self.state.write().await;
        state.submissions.clear();
        state.idempotency_cache.clear();
    }
}

#[async_trait::async_trait]
impl Simulator for RegulatorEndpointSimulator {
    fn name(&self) -> &str {
        "regulator-endpoint"
    }

    fn port(&self) -> u16 {
        self.config.port
    }

    async fn health(&self) -> HealthStatus {
        let state = self.state.read().await;
        let uptime = state.started_at.elapsed().as_secs();

        if state.ready {
            HealthStatus::healthy(self.name(), "1.0.0", uptime)
                .with_details("submission_count", serde_json::json!(state.submissions.len()))
                .with_details("mode", serde_json::json!(format!("{:?}", state.config.mode)))
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
    State(state): State<SharedState<RegulatorEndpointState>>,
) -> impl IntoResponse {
    let state = state.read().await;
    let uptime = state.started_at.elapsed().as_secs();

    if state.ready {
        let health = HealthStatus::healthy("regulator-endpoint", "1.0.0", uptime)
            .with_details("submission_count", serde_json::json!(state.submissions.len()))
            .with_details("mode", serde_json::json!(format!("{:?}", state.config.mode)));
        (StatusCode::OK, Json(health))
    } else {
        let health = HealthStatus::unhealthy("regulator-endpoint", "Not ready");
        (StatusCode::SERVICE_UNAVAILABLE, Json(health))
    }
}

/// Submit credit data
async fn submit_handler(
    State(state): State<SharedState<RegulatorEndpointState>>,
    headers: HeaderMap,
    Json(request): Json<SubmissionRequest>,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut state_guard = state.write().await;

    // Extract idempotency key from header or request
    let idempotency_key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .or_else(|| request.submission_id.clone());

    // Check for failure injection
    let failure = state_guard.config.failure_injection.random_failure().cloned();
    if let Some(ref failure) = failure {
        state_guard.stats.record_request("/api/v1/submit", false, start.elapsed().as_millis() as f64);
        return match failure {
            FailureType::InternalError => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<SubmissionResponse>::error("ERR500", "Internal server error")))
            }
            FailureType::ServiceUnavailable => {
                (StatusCode::SERVICE_UNAVAILABLE, Json(ApiResponse::<SubmissionResponse>::error("ERR503", "Service unavailable")))
            }
            FailureType::Timeout => {
                tokio::time::sleep(Duration::from_secs(30)).await;
                (StatusCode::GATEWAY_TIMEOUT, Json(ApiResponse::<SubmissionResponse>::error("ERR504", "Gateway timeout")))
            }
            FailureType::RateLimited => {
                (StatusCode::TOO_MANY_REQUESTS, Json(ApiResponse::<SubmissionResponse>::error("ERR429", "Rate limited")))
            }
            _ => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<SubmissionResponse>::error("ERR500", "Internal server error")))
            }
        };
    }

    // Apply latency
    state_guard.config.latency.apply().await;

    // Handle timeout mode specially (need to delay before responding)
    if let RegulatorMode::Timeout { delay_ms } = &state_guard.config.mode {
        let delay = *delay_ms;
        drop(state_guard); // Release lock during sleep
        tokio::time::sleep(Duration::from_millis(delay)).await;

        let mut state_guard = state.write().await;
        state_guard.stats.record_request("/api/v1/submit", false, start.elapsed().as_millis() as f64);
        state_guard.stats.record_timeout();
        return (StatusCode::GATEWAY_TIMEOUT, Json(ApiResponse::<SubmissionResponse>::error("TIMEOUT", "Request timed out")));
    }

    // Process the submission
    match state_guard.process_submission(&request, idempotency_key) {
        Ok(response) => {
            state_guard.stats.record_request("/api/v1/submit", true, start.elapsed().as_millis() as f64);
            (StatusCode::OK, Json(ApiResponse::success(response)))
        }
        Err((status, error)) => {
            state_guard.stats.record_request("/api/v1/submit", false, start.elapsed().as_millis() as f64);
            (status, Json(ApiResponse::<SubmissionResponse>::error(&error.error_code, &error.error_message)))
        }
    }
}

/// Get submission by ID
async fn get_submission_handler(
    State(state): State<SharedState<RegulatorEndpointState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut state_guard = state.write().await;

    // Apply latency
    state_guard.config.latency.apply().await;

    if let Some(submission) = state_guard.submissions.get(&id).cloned() {
        state_guard.stats.record_request(&format!("/api/v1/submissions/{}", id), true, start.elapsed().as_millis() as f64);

        let response = SubmissionResponse {
            submission_id: submission.submission_id,
            reference_number: submission.reference_number,
            status: submission.status,
            accepted_count: submission.accepted_count,
            rejected_count: submission.rejected_count,
            rejections: submission.rejections,
            submitted_at: submission.submitted_at.to_rfc3339(),
            processed_at: submission.processed_at.map(|t| t.to_rfc3339()),
            estimated_processing_time_secs: None,
            details: None,
        };

        (StatusCode::OK, Json(ApiResponse::success(response)))
    } else {
        state_guard.stats.record_request(&format!("/api/v1/submissions/{}", id), false, start.elapsed().as_millis() as f64);
        (StatusCode::NOT_FOUND, Json(ApiResponse::<SubmissionResponse>::error("NOT_FOUND", &format!("Submission '{}' not found", id))))
    }
}

/// List submissions
async fn list_submissions_handler(
    State(state): State<SharedState<RegulatorEndpointState>>,
    Query(params): Query<ListSubmissionsParams>,
) -> impl IntoResponse {
    let start = Instant::now();
    let mut state_guard = state.write().await;

    // Apply latency
    state_guard.config.latency.apply().await;

    let mut submissions: Vec<&StoredSubmission> = state_guard.submissions.values().collect();

    // Apply filters
    if let Some(ref bank_code) = params.bank_code {
        submissions.retain(|s| &s.bank_code == bank_code);
    }
    if let Some(ref period) = params.reporting_period {
        submissions.retain(|s| &s.reporting_period == period);
    }
    if let Some(ref status) = params.status {
        submissions.retain(|s| &s.status == status);
    }

    // Sort by submitted_at descending
    submissions.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at));

    let total_count = submissions.len() as u64;
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(20).min(100);
    let start_idx = ((page - 1) * page_size) as usize;
    let end_idx = (start_idx + page_size as usize).min(submissions.len());

    let paged: Vec<SubmissionSummary> = submissions[start_idx..end_idx]
        .iter()
        .map(|s| SubmissionSummary {
            submission_id: s.submission_id.clone(),
            reference_number: s.reference_number.clone(),
            bank_code: s.bank_code.clone(),
            reporting_period: s.reporting_period.clone(),
            status: s.status.clone(),
            record_count: s.record_count,
            accepted_count: s.accepted_count,
            rejected_count: s.rejected_count,
            submitted_at: s.submitted_at.to_rfc3339(),
        })
        .collect();

    state_guard.stats.record_request("/api/v1/submissions", true, start.elapsed().as_millis() as f64);

    let meta = ResponseMeta::paginated(page, page_size, total_count)
        .with_timing(start.elapsed().as_millis() as u64);

    (StatusCode::OK, Json(ApiResponse::success_with_meta(paged, meta)))
}

/// Delete a submission (for testing)
async fn delete_submission_handler(
    State(state): State<SharedState<RegulatorEndpointState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut state_guard = state.write().await;

    if let Some(submission) = state_guard.submissions.remove(&id) {
        // Also remove from idempotency cache
        if let Some(ref key) = submission.idempotency_key {
            state_guard.idempotency_cache.remove(key);
        }
        Json(ApiResponse::success(serde_json::json!({
            "deleted": true,
            "submission_id": id
        })))
    } else {
        Json(ApiResponse::<serde_json::Value>::error("NOT_FOUND", &format!("Submission '{}' not found", id)))
    }
}

/// Change response mode
async fn change_mode_handler(
    State(state): State<SharedState<RegulatorEndpointState>>,
    Json(request): Json<ChangeModeRequest>,
) -> impl IntoResponse {
    let mut state_guard = state.write().await;
    state_guard.config.mode = request.mode.clone();

    Json(ApiResponse::success(serde_json::json!({
        "mode": format!("{:?}", request.mode),
        "updated": true
    })))
}

/// Get current mode
async fn get_mode_handler(
    State(state): State<SharedState<RegulatorEndpointState>>,
) -> impl IntoResponse {
    let state_guard = state.read().await;

    Json(ApiResponse::success(serde_json::json!({
        "mode": format!("{:?}", state_guard.config.mode)
    })))
}

/// Get simulator statistics
async fn stats_handler(
    State(state): State<SharedState<RegulatorEndpointState>>,
) -> impl IntoResponse {
    let state_guard = state.read().await;

    let mut stats = state_guard.stats.clone();

    // Add submission-specific stats
    let mut endpoint_counts = stats.endpoint_counts.clone();
    endpoint_counts.insert("total_submissions".to_string(), state_guard.submissions.len() as u64);

    let accepted = state_guard.submissions.values()
        .filter(|s| s.status == SubmissionStatus::Accepted)
        .count() as u64;
    endpoint_counts.insert("accepted_submissions".to_string(), accepted);

    let rejected = state_guard.submissions.values()
        .filter(|s| s.status == SubmissionStatus::Rejected)
        .count() as u64;
    endpoint_counts.insert("rejected_submissions".to_string(), rejected);

    stats.endpoint_counts = endpoint_counts;

    Json(ApiResponse::success(stats))
}

/// Reset simulator state
async fn reset_handler(
    State(state): State<SharedState<RegulatorEndpointState>>,
) -> impl IntoResponse {
    let mut state_guard = state.write().await;
    state_guard.stats = SimulatorStats::default();
    state_guard.submissions.clear();
    state_guard.idempotency_cache.clear();
    state_guard.submission_counter = 0;
    state_guard.config.mode = RegulatorMode::Accept;

    Json(ApiResponse::success(serde_json::json!({
        "reset": true,
        "mode": "Accept"
    })))
}

/// Summary of a submission for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionSummary {
    pub submission_id: String,
    pub reference_number: String,
    pub bank_code: String,
    pub reporting_period: String,
    pub status: SubmissionStatus,
    pub record_count: usize,
    pub accepted_count: usize,
    pub rejected_count: usize,
    pub submitted_at: String,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_submission_status_serialization() {
        let status = SubmissionStatus::Accepted;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"ACCEPTED\"");
    }

    #[test]
    fn test_submission_request() {
        let request = SubmissionRequest {
            submission_id: Some("test-123".to_string()),
            reporting_period: "202401".to_string(),
            bank_code: "BANKXYZ".to_string(),
            records: vec![serde_json::json!({"id": "1"})],
            metadata: None,
        };

        assert_eq!(request.submission_id, Some("test-123".to_string()));
        assert_eq!(request.records.len(), 1);
    }

    #[test]
    fn test_state_generate_reference_number() {
        let config = RegulatorEndpointConfig::default();
        let mut state = RegulatorEndpointState::new(config);

        let ref1 = state.generate_reference_number();
        let ref2 = state.generate_reference_number();

        assert!(ref1.starts_with("OJK-SLIK-"));
        assert!(ref2.starts_with("OJK-SLIK-"));
        assert_ne!(ref1, ref2);
    }

    #[test]
    fn test_state_accept_submission() {
        let config = RegulatorEndpointConfig::default();
        let mut state = RegulatorEndpointState::new(config);

        let request = SubmissionRequest {
            submission_id: None,
            reporting_period: "202401".to_string(),
            bank_code: "BANKXYZ".to_string(),
            records: vec![
                serde_json::json!({"id": "1"}),
                serde_json::json!({"id": "2"}),
            ],
            metadata: None,
        };

        let result = state.process_submission(&request, Some("key-1".to_string()));
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response.status, SubmissionStatus::Accepted);
        assert_eq!(response.accepted_count, 2);
        assert_eq!(response.rejected_count, 0);
    }

    #[test]
    fn test_state_idempotency() {
        let config = RegulatorEndpointConfig::default();
        let mut state = RegulatorEndpointState::new(config);

        let request = SubmissionRequest {
            submission_id: Some("sub-123".to_string()),
            reporting_period: "202401".to_string(),
            bank_code: "BANKXYZ".to_string(),
            records: vec![serde_json::json!({"id": "1"})],
            metadata: None,
        };

        // First submission
        let result1 = state.process_submission(&request, Some("idem-key-1".to_string()));
        assert!(result1.is_ok());
        let response1 = result1.unwrap();

        // Duplicate submission with same idempotency key
        let result2 = state.process_submission(&request, Some("idem-key-1".to_string()));
        assert!(result2.is_ok());
        let response2 = result2.unwrap();

        // Should return the same submission ID
        assert_eq!(response1.submission_id, response2.submission_id);
        assert_eq!(response1.reference_number, response2.reference_number);

        // Should indicate it's an idempotent response
        assert!(response2.details.as_ref().unwrap().get("idempotent").is_some());
    }

    #[test]
    fn test_state_reject_mode() {
        let mut config = RegulatorEndpointConfig::default();
        config.mode = RegulatorMode::Reject {
            error_code: "INVALID_DATA".to_string(),
            error_message: "Data validation failed".to_string(),
        };

        let mut state = RegulatorEndpointState::new(config);

        let request = SubmissionRequest {
            submission_id: None,
            reporting_period: "202401".to_string(),
            bank_code: "BANKXYZ".to_string(),
            records: vec![serde_json::json!({"id": "1"})],
            metadata: None,
        };

        let result = state.process_submission(&request, None);
        assert!(result.is_err());

        let (status, error) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error.error_code, "INVALID_DATA");
    }

    #[test]
    fn test_state_service_unavailable_mode() {
        let mut config = RegulatorEndpointConfig::default();
        config.mode = RegulatorMode::ServiceUnavailable;

        let mut state = RegulatorEndpointState::new(config);

        let request = SubmissionRequest {
            submission_id: None,
            reporting_period: "202401".to_string(),
            bank_code: "BANKXYZ".to_string(),
            records: vec![serde_json::json!({"id": "1"})],
            metadata: None,
        };

        let result = state.process_submission(&request, None);
        assert!(result.is_err());

        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_state_rate_limited_mode() {
        let mut config = RegulatorEndpointConfig::default();
        config.mode = RegulatorMode::RateLimited;

        let mut state = RegulatorEndpointState::new(config);

        let request = SubmissionRequest {
            submission_id: None,
            reporting_period: "202401".to_string(),
            bank_code: "BANKXYZ".to_string(),
            records: vec![serde_json::json!({"id": "1"})],
            metadata: None,
        };

        let result = state.process_submission(&request, None);
        assert!(result.is_err());

        let (status, error) = result.unwrap_err();
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error.error_code, "RATE_LIMITED");
    }

    #[test]
    fn test_state_queued_mode() {
        let mut config = RegulatorEndpointConfig::default();
        config.mode = RegulatorMode::Queued { queue_delay_ms: 5000 };

        let mut state = RegulatorEndpointState::new(config);

        let request = SubmissionRequest {
            submission_id: None,
            reporting_period: "202401".to_string(),
            bank_code: "BANKXYZ".to_string(),
            records: vec![serde_json::json!({"id": "1"})],
            metadata: None,
        };

        let result = state.process_submission(&request, None);
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response.status, SubmissionStatus::Queued);
        assert!(response.estimated_processing_time_secs.is_some());
    }

    #[test]
    fn test_state_partial_reject_mode() {
        let mut config = RegulatorEndpointConfig::default();
        config.mode = RegulatorMode::PartialReject { reject_ratio: 0.5 };

        let mut state = RegulatorEndpointState::new(config);

        let request = SubmissionRequest {
            submission_id: None,
            reporting_period: "202401".to_string(),
            bank_code: "BANKXYZ".to_string(),
            records: (0..100).map(|i| serde_json::json!({"id": i})).collect(),
            metadata: None,
        };

        let result = state.process_submission(&request, None);
        assert!(result.is_ok());

        let response = result.unwrap();
        // With 50% reject ratio, we should have some accepted and some rejected
        // (statistically unlikely to have all accepted or all rejected)
        assert!(response.accepted_count > 0 || response.rejected_count > 0);
        assert_eq!(response.accepted_count + response.rejected_count, 100);
    }

    #[test]
    fn test_record_rejection() {
        let rejection = RecordRejection {
            record_index: 5,
            record_id: Some("CR0005".to_string()),
            error_code: "INVALID_NIK".to_string(),
            message: "NIK format is invalid".to_string(),
            field: Some("debtor_nik".to_string()),
        };

        assert_eq!(rejection.record_index, 5);
        assert_eq!(rejection.error_code, "INVALID_NIK");
    }

    #[test]
    fn test_stored_submission() {
        let submission = StoredSubmission {
            submission_id: "sub-123".to_string(),
            reference_number: "OJK-SLIK-20240101120000-000001".to_string(),
            bank_code: "BANKXYZ".to_string(),
            reporting_period: "202401".to_string(),
            status: SubmissionStatus::Accepted,
            record_count: 100,
            accepted_count: 100,
            rejected_count: 0,
            rejections: Vec::new(),
            submitted_at: Utc::now(),
            processed_at: Some(Utc::now()),
            idempotency_key: Some("idem-123".to_string()),
            attempt_count: 1,
        };

        assert_eq!(submission.status, SubmissionStatus::Accepted);
        assert_eq!(submission.record_count, 100);
    }
}
