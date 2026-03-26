//! # Engine Simulator for Paket B Tests
//!
//! This module simulates the Vastar-Flow Engine for Submission workflow testing.
//! It provides webhook endpoints, submission orchestration, and integrates with
//! the Regulator Endpoint Simulator.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ═══════════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════════

const REGULATOR_ENDPOINT_URL: &str = "http://127.0.0.1:18084";

// ═══════════════════════════════════════════════════════════════════════════════
// Data Structures (matching test expectations)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionRequest {
    pub submission_id: String,
    pub locked_dataset_version: String,
    pub artifact_format: String,
    pub options: SubmissionOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionOptions {
    pub retry_policy: RetryPolicy,
    #[serde(default)]
    pub off_peak_window: Option<OffPeakWindow>,
    #[serde(default)]
    pub batch_size: Option<u64>,
    #[serde(default)]
    pub include_evidence_pack: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(alias = "initial_delay_ms", alias = "initial_backoff_ms", default = "default_initial_delay")]
    pub initial_backoff_ms: u64,
    #[serde(alias = "max_delay_ms", alias = "max_backoff_ms", default = "default_max_delay")]
    pub max_backoff_ms: u64,
    #[serde(default = "default_multiplier")]
    pub backoff_multiplier: f64,
    #[serde(default = "default_send_to_dlq")]
    pub send_to_dlq: bool,
}

fn default_max_attempts() -> u32 { 3 }
fn default_initial_delay() -> u64 { 1000 }
fn default_max_delay() -> u64 { 30000 }
fn default_multiplier() -> f64 { 2.0 }
fn default_send_to_dlq() -> bool { true }

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff_ms: 1000,
            max_backoff_ms: 30000,
            backoff_multiplier: 2.0,
            send_to_dlq: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffPeakWindow {
    pub start_hour: u8,
    pub end_hour: u8,
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionResponse {
    pub execution_id: String,
    pub submission_id: String,
    pub status: SubmissionStatus,
    pub artifact: Option<Artifact>,
    pub attempts: Vec<SubmissionAttempt>,
    pub evidence_pack: Option<EvidencePack>,
    pub audit_events: Vec<AuditEvent>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SubmissionStatus {
    Pending,
    Generating,
    Submitting,
    Submitted,
    Acknowledged,
    Rejected,
    PendingRetry,
    Failed,
    Escalated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: String,
    pub artifact_hash: String,
    pub format: String,
    pub row_count: u64,
    pub file_size: u64,
    pub locked_dataset_version: String,
    pub created_at: String,
    pub precheck_result: Option<PrecheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecheckResult {
    pub passed: bool,
    pub checks: Vec<PrecheckItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecheckItem {
    pub check_name: String,
    pub passed: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionAttempt {
    pub attempt_number: u32,
    pub timestamp: String,
    pub status: AttemptStatus,
    pub response_code: Option<u16>,
    pub response_message: Option<String>,
    pub correlation_id: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AttemptStatus {
    Success,
    Timeout,
    Rejected,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePack {
    pub pack_id: String,
    pub created_at: String,
    pub contents: EvidenceContents,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceContents {
    pub run_id: String,
    pub dataset_version: String,
    pub artifact_hash: String,
    pub submission_attempts: Vec<SubmissionAttempt>,
    pub audit_log_hash: String,
    pub approval_trail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_type: String,
    pub timestamp: String,
    pub actor: String,
    pub details: serde_json::Value,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Engine State
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug)]
pub struct EngineState {
    pub submissions: RwLock<HashMap<String, SubmissionResponse>>,
    pub dlq: RwLock<Vec<DlqEntry>>,
    pub start_time: std::time::Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqEntry {
    pub submission_id: String,
    pub reason: String,
    pub timestamp: String,
    pub attempts: u32,
    pub last_error: String,
}

impl EngineState {
    pub fn new() -> Self {
        Self {
            submissions: RwLock::new(HashMap::new()),
            dlq: RwLock::new(Vec::new()),
            start_time: std::time::Instant::now(),
        }
    }
}

impl Default for EngineState {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Engine Simulator
// ═══════════════════════════════════════════════════════════════════════════════

pub struct EngineSimulator {
    state: Arc<EngineState>,
}

impl EngineSimulator {
    pub fn new() -> Self {
        Self {
            state: Arc::new(EngineState::new()),
        }
    }

    pub fn router(&self) -> Router {
        let state = self.state.clone();

        Router::new()
            // Health endpoints
            .route("/health", get(health_check))
            // Webhook endpoint for submissions
            .route("/webhook/:tenant_id/:workflow_id/:trigger_id", post(handle_webhook))
            // DLQ management
            .route("/internal/dlq", get(get_dlq_entries))
            .route("/internal/dlq/:submission_id/retry", post(retry_dlq_entry))
            // Submission status
            .route("/submissions/:submission_id", get(get_submission_status))
            .with_state(state)
    }

    pub fn state(&self) -> Arc<EngineState> {
        self.state.clone()
    }
}

impl Default for EngineSimulator {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Admin Simulator (for health check on port 9090)
// ═══════════════════════════════════════════════════════════════════════════════

pub struct AdminSimulator {
    state: Arc<EngineState>,
}

impl AdminSimulator {
    pub fn new(state: Arc<EngineState>) -> Self {
        Self { state }
    }

    pub fn router(&self) -> Router {
        let state = self.state.clone();

        Router::new()
            .route("/health", get(admin_health_check))
            .route("/metrics", get(metrics))
            .with_state(state)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Handlers
// ═══════════════════════════════════════════════════════════════════════════════

async fn health_check(State(state): State<Arc<EngineState>>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();
    Json(serde_json::json!({
        "status": "healthy",
        "service": "engine-simulator",
        "version": "1.0.0",
        "uptime_secs": uptime
    }))
}

async fn admin_health_check(State(state): State<Arc<EngineState>>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();
    Json(serde_json::json!({
        "status": "healthy",
        "service": "admin-simulator",
        "version": "1.0.0",
        "uptime_secs": uptime
    }))
}

async fn metrics(State(state): State<Arc<EngineState>>) -> impl IntoResponse {
    let submissions = state.submissions.read().await;
    let dlq = state.dlq.read().await;
    
    Json(serde_json::json!({
        "total_submissions": submissions.len(),
        "dlq_size": dlq.len(),
        "uptime_secs": state.start_time.elapsed().as_secs()
    }))
}

async fn handle_webhook(
    State(state): State<Arc<EngineState>>,
    Path((_tenant_id, _workflow_id, _trigger_id)): Path<(String, String, String)>,
    Json(request): Json<SubmissionRequest>,
) -> impl IntoResponse {
    let start = std::time::Instant::now();
    let submission_id = request.submission_id.clone();
    let execution_id = format!("EXEC-{}", Uuid::new_v4().to_string()[..8].to_uppercase());

    // Generate artifact
    let artifact = generate_artifact(&request);

    // Attempt submission to regulator
    let (status, attempts, error_reason) = submit_to_regulator(&request, &artifact).await;

    // Generate audit events
    let audit_events = generate_audit_events(&submission_id, &status, &attempts);

    // Generate evidence pack if requested
    let evidence_pack = if request.options.include_evidence_pack {
        Some(generate_evidence_pack(&request, &artifact, &attempts))
    } else {
        None
    };

    let response = SubmissionResponse {
        execution_id: execution_id.clone(),
        submission_id: submission_id.clone(),
        status,
        artifact: Some(artifact),
        attempts,
        evidence_pack,
        audit_events,
        duration_ms: start.elapsed().as_millis() as u64,
    };

    // Store in state
    {
        let mut submissions = state.submissions.write().await;
        submissions.insert(submission_id.clone(), response.clone());
    }

    // Add to DLQ if failed
    if status == SubmissionStatus::Escalated || status == SubmissionStatus::Failed {
        let mut dlq = state.dlq.write().await;
        dlq.push(DlqEntry {
            submission_id: submission_id.clone(),
            reason: error_reason.unwrap_or_else(|| "Unknown".to_string()),
            timestamp: Utc::now().to_rfc3339(),
            attempts: response.attempts.len() as u32,
            last_error: response.attempts.last()
                .and_then(|a| a.response_message.clone())
                .unwrap_or_else(|| "No error".to_string()),
        });
    }

    (StatusCode::OK, Json(response))
}

async fn get_dlq_entries(State(state): State<Arc<EngineState>>) -> impl IntoResponse {
    let dlq = state.dlq.read().await;
    Json(dlq.clone())
}

async fn retry_dlq_entry(
    State(state): State<Arc<EngineState>>,
    Path(submission_id): Path<String>,
) -> impl IntoResponse {
    let mut dlq = state.dlq.write().await;
    
    // Find and remove from DLQ
    if let Some(pos) = dlq.iter().position(|e| e.submission_id == submission_id) {
        dlq.remove(pos);
        (StatusCode::OK, Json(serde_json::json!({"status": "retrying", "submission_id": submission_id})))
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Entry not found in DLQ"})))
    }
}

async fn get_submission_status(
    State(state): State<Arc<EngineState>>,
    Path(submission_id): Path<String>,
) -> impl IntoResponse {
    let submissions = state.submissions.read().await;
    
    if let Some(submission) = submissions.get(&submission_id) {
        (StatusCode::OK, Json(serde_json::json!(submission)))
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Submission not found"})))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════════════════════════

fn generate_artifact(request: &SubmissionRequest) -> Artifact {
    let artifact_id = format!("ART-{}", Uuid::new_v4().to_string()[..8].to_uppercase());
    let content = format!("{}:{}:{}", request.submission_id, request.locked_dataset_version, Utc::now());
    let artifact_hash = format!("{:x}", md5::compute(content.as_bytes()));

    Artifact {
        artifact_id,
        artifact_hash,
        format: request.artifact_format.clone(),
        row_count: 1000, // Mock row count
        file_size: 1024 * 50, // 50KB mock size
        locked_dataset_version: request.locked_dataset_version.clone(),
        created_at: Utc::now().to_rfc3339(),
        precheck_result: Some(PrecheckResult {
            passed: true,
            checks: vec![
                PrecheckItem {
                    check_name: "schema_validation".to_string(),
                    passed: true,
                    message: None,
                },
                PrecheckItem {
                    check_name: "hash_verification".to_string(),
                    passed: true,
                    message: None,
                },
                PrecheckItem {
                    check_name: "size_limit".to_string(),
                    passed: true,
                    message: None,
                },
            ],
        }),
    }
}

async fn submit_to_regulator(
    request: &SubmissionRequest,
    artifact: &Artifact,
) -> (SubmissionStatus, Vec<SubmissionAttempt>, Option<String>) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    let max_attempts = request.options.retry_policy.max_attempts;
    let mut attempts = Vec::new();
    let mut last_error: Option<String> = None;

    for attempt_num in 1..=max_attempts {
        let attempt_start = std::time::Instant::now();
        let correlation_id = format!("CORR-{}", Uuid::new_v4().to_string()[..8].to_uppercase());
        
        let submit_url = format!("{}/api/v1/submit", REGULATOR_ENDPOINT_URL);
        
        // Generate mock records (Regulator expects records array)
        let mock_records: Vec<serde_json::Value> = (0..artifact.row_count.min(10))
            .map(|i| serde_json::json!({
                "id": format!("REC-{:06}", i),
                "nik": format!("32050219900{:05}", i),
                "nama_lengkap": format!("Test User {}", i),
                "jenis_fasilitas": "KMK",
                "jumlah_kredit": 10000000,
                "mata_uang": "IDR",
                "suku_bunga": 12.5,
                "tanggal_mulai": "2024-01-01",
                "tanggal_jatuh_tempo": "2025-01-01",
                "saldo_outstanding": 8000000,
                "kolektabilitas": 1
            }))
            .collect();
        
        let result = client
            .post(&submit_url)
            .json(&serde_json::json!({
                "submission_id": request.submission_id,
                "reporting_period": "202501",
                "bank_code": "BANK001",
                "records": mock_records,
                "metadata": {
                    "artifact_hash": artifact.artifact_hash,
                    "format": artifact.format,
                    "dataset_version": request.locked_dataset_version
                }
            }))
            .send()
            .await;

        match result {
            Ok(response) => {
                let status_code = response.status().as_u16();
                let duration = attempt_start.elapsed().as_millis() as u64;

                if response.status().is_success() {
                    attempts.push(SubmissionAttempt {
                        attempt_number: attempt_num,
                        timestamp: Utc::now().to_rfc3339(),
                        status: AttemptStatus::Success,
                        response_code: Some(status_code),
                        response_message: None,
                        correlation_id,
                        duration_ms: duration,
                    });
                    return (SubmissionStatus::Acknowledged, attempts, None);
                } else if status_code == 400 || status_code == 422 {
                    // Rejection - don't retry
                    let body = response.text().await.unwrap_or_default();
                    attempts.push(SubmissionAttempt {
                        attempt_number: attempt_num,
                        timestamp: Utc::now().to_rfc3339(),
                        status: AttemptStatus::Rejected,
                        response_code: Some(status_code),
                        response_message: Some(body.clone()),
                        correlation_id,
                        duration_ms: duration,
                    });
                    return (SubmissionStatus::Rejected, attempts, Some(body));
                } else {
                    // Server error - retry
                    let body = response.text().await.unwrap_or_default();
                    last_error = Some(body.clone());
                    attempts.push(SubmissionAttempt {
                        attempt_number: attempt_num,
                        timestamp: Utc::now().to_rfc3339(),
                        status: AttemptStatus::Error,
                        response_code: Some(status_code),
                        response_message: Some(body),
                        correlation_id,
                        duration_ms: duration,
                    });
                }
            }
            Err(e) => {
                let duration = attempt_start.elapsed().as_millis() as u64;
                let (error_status, error_msg) = if e.is_timeout() {
                    (AttemptStatus::Timeout, "Request timeout".to_string())
                } else if e.is_connect() {
                    (AttemptStatus::Error, "Connection failed".to_string())
                } else {
                    (AttemptStatus::Error, format!("Request error: {}", e))
                };
                
                last_error = Some(error_msg.clone());
                attempts.push(SubmissionAttempt {
                    attempt_number: attempt_num,
                    timestamp: Utc::now().to_rfc3339(),
                    status: error_status,
                    response_code: None,
                    response_message: Some(error_msg),
                    correlation_id,
                    duration_ms: duration,
                });
            }
        }

        // Wait before retry (exponential backoff)
        if attempt_num < max_attempts {
            let delay = calculate_backoff(attempt_num, &request.options.retry_policy);
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
    }

    // All attempts failed
    if attempts.len() as u32 >= max_attempts {
        (SubmissionStatus::Escalated, attempts, last_error)
    } else {
        (SubmissionStatus::Failed, attempts, last_error)
    }
}

fn calculate_backoff(attempt: u32, policy: &RetryPolicy) -> u64 {
    let delay = (policy.initial_backoff_ms as f64) * policy.backoff_multiplier.powi(attempt as i32 - 1);
    (delay as u64).min(policy.max_backoff_ms)
}

fn generate_audit_events(
    submission_id: &str,
    status: &SubmissionStatus,
    attempts: &[SubmissionAttempt],
) -> Vec<AuditEvent> {
    let mut events = vec![
        AuditEvent {
            event_type: "submission_started".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            actor: "engine".to_string(),
            details: serde_json::json!({
                "submission_id": submission_id
            }),
        },
        // Artifact generated event
        AuditEvent {
            event_type: "artifact_generated".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            actor: "engine".to_string(),
            details: serde_json::json!({
                "submission_id": submission_id,
                "format": "XML"
            }),
        },
    ];

    for attempt in attempts {
        let attempt_event_type = match attempt.status {
            AttemptStatus::Success => "attempt_success".to_string(),
            AttemptStatus::Timeout => "attempt_timeout".to_string(),
            AttemptStatus::Rejected => "attempt_rejected".to_string(),
            AttemptStatus::Error => "attempt_error".to_string(),
        };
        
        events.push(AuditEvent {
            event_type: attempt_event_type,
            timestamp: attempt.timestamp.clone(),
            actor: "engine".to_string(),
            details: serde_json::json!({
                "attempt": attempt.attempt_number,
                "duration_ms": attempt.duration_ms,
                "response_code": attempt.response_code,
                "correlation_id": attempt.correlation_id
            }),
        });
    }

    // Final status event
    let final_event_type = match status {
        SubmissionStatus::Acknowledged | SubmissionStatus::Submitted => "submission_success".to_string(),
        SubmissionStatus::Rejected => "submission_rejected".to_string(),
        SubmissionStatus::Escalated => "submission_escalated".to_string(),
        SubmissionStatus::Failed => "submission_failed".to_string(),
        _ => format!("submission_{:?}", status).to_lowercase(),
    };
    
    events.push(AuditEvent {
        event_type: final_event_type,
        timestamp: Utc::now().to_rfc3339(),
        actor: "engine".to_string(),
        details: serde_json::json!({
            "final_status": format!("{:?}", status),
            "total_attempts": attempts.len()
        }),
    });

    events
}

fn generate_evidence_pack(
    request: &SubmissionRequest,
    artifact: &Artifact,
    attempts: &[SubmissionAttempt],
) -> EvidencePack {
    let pack_id = format!("EVP-{}", Uuid::new_v4().to_string()[..8].to_uppercase());
    
    let contents = EvidenceContents {
        run_id: request.submission_id.clone(),
        dataset_version: request.locked_dataset_version.clone(),
        artifact_hash: artifact.artifact_hash.clone(),
        submission_attempts: attempts.to_vec(),
        audit_log_hash: format!("{:x}", md5::compute(format!("{:?}", attempts).as_bytes())),
        approval_trail: None,
    };

    let hash = format!("{:x}", md5::compute(format!("{:?}", contents).as_bytes()));

    EvidencePack {
        pack_id,
        created_at: Utc::now().to_rfc3339(),
        contents,
        hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_retry_policy() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.initial_backoff_ms, 1000);
    }

    #[test]
    fn test_calculate_backoff() {
        let policy = RetryPolicy::default();
        
        let delay1 = calculate_backoff(1, &policy);
        assert_eq!(delay1, 1000);
        
        let delay2 = calculate_backoff(2, &policy);
        assert_eq!(delay2, 2000);
        
        let delay3 = calculate_backoff(3, &policy);
        assert_eq!(delay3, 4000);
    }
}
