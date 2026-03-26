//! # Core Banking Simulator
//!
//! Simulates a Core Banking data source that provides credit data for SLIK reporting.
//! Supports:
//! - Deterministic dataset generation (via POST /generate)
//! - Read-only listing (GET /credits) without regenerating
//! - Cutoff-window filtering (cutoff_start/cutoff_end) on last_updated
//! - Pagination + cursor style (next_cursor)
//! - SSE streaming (for PoC) + NDJSON streaming (for throughput testing)
//!
//! ## Endpoints
//! - `GET /health` - Health check
//! - `GET /api/v1/credits` - List credit records (paginated, read-only)
//! - `GET /api/v1/credits/:id` - Get record by id
//! - `GET /api/v1/credits/count` - Count records (optionally filtered)
//! - `POST /api/v1/credits/generate` - Generate/replace/append dataset (ONLY mutating op)
//! - `GET /api/v1/credits/stream` - Stream generated records (SSE) (stateless generator)
//! - `GET /api/v1/credits/ndjson` - Stream generated records (NDJSON) (stateless generator)
//! - `GET /api/v1/stats` - Simulator statistics
//! - `POST /api/v1/reset` - Reset statistics and data
//! - `PUT /api/v1/config` - Update configuration dynamically

use crate::{
    config::{CoreBankingConfig, FailureType, LatencyConfig},
    models::credit::CreditRecord,
    shared_state, ApiResponse, HealthStatus, ResponseMeta, SharedState, Simulator,
    SimulatorError, SimulatorResult, SimulatorStats,
};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{sse::Event, IntoResponse, Json, Response, Sse},
    routing::{get, post, put},
    Router,
};

use chrono::{NaiveDate, NaiveDateTime, Utc};
use futures::stream::Stream;
use futures::StreamExt;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_util::io::StreamReader;
use tokio::io::AsyncBufReadExt;
use std::{
    convert::Infallible,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

// ============================================================================
// Helpers
// ============================================================================

fn parse_date_opt(s: &Option<String>) -> Result<Option<NaiveDate>, &'static str> {
    if let Some(v) = s {
        let d = NaiveDate::parse_from_str(v, "%Y-%m-%d").map_err(|_| "invalid date format")?;
        Ok(Some(d))
    } else {
        Ok(None)
    }
}

fn parse_dt_opt(s: &Option<String>) -> Result<Option<NaiveDateTime>, &'static str> {
    if let Some(v) = s {
        // Accept RFC3339 with Z, minimal for PoC.
        let dt = chrono::DateTime::parse_from_rfc3339(v)
            .map_err(|_| "invalid datetime format")?
            .naive_utc();
        Ok(Some(dt))
    } else {
        Ok(None)
    }
}

fn within_cutoff(last_updated_rfc3339: &str, start: Option<NaiveDate>, end: Option<NaiveDate>) -> bool {
    // Parse last_updated. If parsing fails, treat as NOT within window.
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(last_updated_rfc3339) else {
        return false;
    };
    let d = dt.date_naive();
    if let Some(s) = start {
        if d < s {
            return false;
        }
    }
    if let Some(e) = end {
        if d > e {
            return false;
        }
    }
    true
}

// Cursor is a simple numeric offset for PoC.
fn parse_cursor(cursor: &Option<String>) -> usize {
    cursor
        .as_ref()
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0)
}

fn next_cursor_if_any(current_offset: usize, returned: usize, total: usize) -> Option<String> {
    let next = current_offset.saturating_add(returned);
    if next < total {
        Some(next.to_string())
    } else {
        None
    }
}

// CreditRecord models moved to crate::models::credit

/// Request parameters for credit listing
#[derive(Debug, Deserialize)]
pub struct ListCreditsParams {
    pub page: Option<u32>,
    pub page_size: Option<u32>,

    /// Cursor offset for stable paging (optional). If present, `page` is ignored.
    pub cursor: Option<String>,

    /// Cutoff window (inclusive) for filtering by `last_updated` date.
    pub cutoff_start: Option<String>,
    pub cutoff_end: Option<String>,

    /// Mode: "normal" (default) or "stream"
    pub mode: Option<String>,

    /// Chunk size for streaming (number of records per flush)
    pub stream_chunk_size: Option<usize>,
}

/// Request parameters for generating credits (mutating)
#[derive(Debug, Deserialize)]
pub struct GenerateCreditsRequest {
    pub count: u32,
    pub dirty_ratio: Option<f64>,
    pub seed: Option<u64>,
    pub append: Option<bool>,
}

/// Request parameters for loading specific credits from a file (mutating)
#[derive(Debug, Deserialize)]
pub struct LoadCreditsRequest {
    pub records: Vec<CreditRecord>,
    pub append: Option<bool>,
}

/// Request parameters for streaming credits (stateless generator)
#[derive(Debug, Deserialize)]
pub struct StreamCreditsParams {
    pub count: Option<u32>,
    pub dirty_ratio: Option<f64>,
    pub seed: Option<u64>,
    pub batch_size: Option<u32>,
    pub delay_ms: Option<u64>,
}

/// Dynamic configuration update request
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    pub dirty_ratio: Option<f64>,
    pub latency_ms: Option<u64>,
    pub failure_rate: Option<f64>,
}

// ============================================================================
// Simulator State
// ============================================================================

/// Internal state for Core Banking Simulator
pub struct CoreBankingState {
    pub config: CoreBankingConfig,
    pub records: std::collections::HashMap<String, CreditRecord>,
    pub stats: SimulatorStats,
    pub started_at: Instant,
    pub ready: bool,

    // current dataset metadata (for reproducibility)
    pub dataset_seed: Option<u64>,
    pub dataset_dirty_ratio: f64,
}

impl CoreBankingState {
    pub fn new(config: CoreBankingConfig) -> Self {
        Self {
            dataset_seed: config.seed,
            dataset_dirty_ratio: config.default_dirty_ratio,
            config,
            records: std::collections::HashMap::new(),
            stats: SimulatorStats::default(),
            started_at: Instant::now(),
            ready: false,
        }
    }

    /// Generate records based on configuration
    pub fn generate_records(
        count: u32,
        dirty_ratio: f64,
        seed: Option<u64>,
        start_id: u64,
    ) -> Vec<CreditRecord> {
        let mut rng: ChaCha8Rng = if let Some(s) = seed {
            ChaCha8Rng::seed_from_u64(s)
        } else {
            ChaCha8Rng::from_rng(rand::thread_rng()).unwrap()
        };

        let error_types = [
            "invalid_nik",
            "negative_amount",
            "invalid_date",
            "missing_field",
            "invalid_currency",
            "invalid_collectability",
            "outstanding_gt_plafon",
        ];

        let mut records = Vec::with_capacity(count as usize);
        let error_threshold = (dirty_ratio.clamp(0.0, 1.0) * u64::MAX as f64) as u64;

        for i in 0..count {
            let mut record = CreditRecord::generate_clean(&mut rng, start_id + i as u64);

            if error_threshold > 0 && rng.gen::<u64>() < error_threshold {
                let error_type = error_types[rng.gen_range(0..error_types.len())];
                record = record.inject_error(&mut rng, error_type);
            }

            records.push(record);
        }

        records
    }
}

// ============================================================================
// Core Banking Simulator
// ============================================================================

/// Core Banking Simulator implementation
pub struct CoreBankingSimulator {
    state: SharedState<CoreBankingState>,
    config: CoreBankingConfig,
}

impl CoreBankingSimulator {
    pub fn new(config: CoreBankingConfig) -> Self {
        let state = shared_state(CoreBankingState::new(config.clone()));
        Self { state, config }
    }

    pub async fn run(&self, shutdown_rx: oneshot::Receiver<()>) -> SimulatorResult<()> {
        // Initialize with default records
        {
            let (record_count, dirty_ratio, seed) = {
                let state = self.state.read().await;
                (
                    state.config.default_record_count,
                    state.config.default_dirty_ratio,
                    state.config.seed,
                )
            };

            // Generate records outside the lock (CPU intensive)
            let records =
                CoreBankingState::generate_records(record_count, dirty_ratio, seed, 0);

            let mut state = self.state.write().await;
            for record in records {
                state.records.insert(record.id.clone(), record);
            }
            state.ready = true;
            state.dataset_seed = seed;
            state.dataset_dirty_ratio = dirty_ratio;
        }

        let app = self.create_router();
        let addr: std::net::SocketAddr = self
            .config
            .socket_addr()
            .parse()
            .map_err(|e| SimulatorError::ConfigError(format!("Invalid address: {e}")))?;

        tracing::info!("Core Banking Simulator listening on {addr}");

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| SimulatorError::BindError(e.to_string()))?;

        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
                tracing::info!("Core Banking Simulator shutting down");
            })
            .await
            .map_err(|e| SimulatorError::StartError(e.to_string()))?;

        Ok(())
    }

    fn create_router(&self) -> Router {
        let state = self.state.clone();

        Router::new()
            .route("/health", get(health_handler))
            .route("/api/v1/credits", get(list_credits_handler).post(list_credits_handler))
            .route("/api/v1/credits/:id", get(get_credit_handler))
            .route("/api/v1/credits/count", get(count_credits_handler))
            .route("/api/v1/credits/generate", post(generate_credits_handler))
            .route("/api/v1/credits/load", post(load_credits_handler))
            .route("/api/v1/credits/load-ndjson", post(load_ndjson_handler))
            .route("/api/v1/credits/stream", get(stream_credits_handler))
            .route("/api/v1/credits/ndjson", get(ndjson_credits_handler))
            .route("/api/v1/stats", get(stats_handler))
            .route("/api/v1/reset", post(reset_handler))
            .route("/api/v1/config", put(update_config_handler))
            .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
            .with_state(state)
    }

    pub async fn get_records(&self) -> Vec<CreditRecord> {
        self.state.read().await.records.values().cloned().collect()
    }

    pub async fn set_records(&self, records: Vec<CreditRecord>) {
        let mut state = self.state.write().await;
        state.records.clear();
        for record in records {
            state.records.insert(record.id.clone(), record);
        }
    }
}

#[async_trait::async_trait]
impl Simulator for CoreBankingSimulator {
    fn name(&self) -> &str {
        "core-banking"
    }

    fn port(&self) -> u16 {
        self.config.port
    }

    async fn health(&self) -> HealthStatus {
        let state = self.state.read().await;
        let uptime = state.started_at.elapsed().as_secs();

        if state.ready {
            HealthStatus::healthy(self.name(), "1.0.0", uptime)
                .with_details("record_count", serde_json::json!(state.records.len()))
                .with_details("dataset_seed", serde_json::json!(state.dataset_seed))
                .with_details("dataset_dirty_ratio", serde_json::json!(state.dataset_dirty_ratio))
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

async fn health_handler(State(state): State<SharedState<CoreBankingState>>) -> impl IntoResponse {
    let state = state.read().await;
    let uptime = state.started_at.elapsed().as_secs();

    if state.ready {
        let health = HealthStatus::healthy("core-banking", "1.0.0", uptime)
            .with_details("record_count", serde_json::json!(state.records.len()))
            .with_details("dataset_seed", serde_json::json!(state.dataset_seed))
            .with_details("dataset_dirty_ratio", serde_json::json!(state.dataset_dirty_ratio));
        (StatusCode::OK, Json(health))
    } else {
        let health = HealthStatus::unhealthy("core-banking", "Not ready");
        (StatusCode::SERVICE_UNAVAILABLE, Json(health))
    }
}

/// List credits (READ ONLY) with pagination + cutoff filtering
async fn list_credits_handler(
    State(state): State<SharedState<CoreBankingState>>,
    Query(params): Query<ListCreditsParams>,
) -> Response {
    let start = Instant::now();

    // Parse cutoff params early (fail fast)
    let cutoff_start = match parse_date_opt(&params.cutoff_start) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::<Vec<CreditRecord>>::error("BAD_REQUEST", e))).into_response(),
    };
    let cutoff_end = match parse_date_opt(&params.cutoff_end) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::<Vec<CreditRecord>>::error("BAD_REQUEST", e))).into_response(),
    };

    // 1. Read lock for config and limits
    let (failure, latency_config, max_records) = {
        let state = state.read().await;
        (
            state.config.failure_injection.random_failure().cloned(),
            state.config.latency.clone(),
            state.config.max_records_per_request,
        )
    };

    // Failure injection (optional)
    if let Some(ref failure) = failure {
        // Need write lock to record stats
        let mut state = state.write().await;
        state
            .stats
            .record_request("/api/v1/credits", false, start.elapsed().as_millis() as f64);
        return match failure {
            FailureType::InternalError => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error("INTERNAL_ERROR", "Simulated internal error")),
            ).into_response(),
            FailureType::Timeout => {
                tokio::time::sleep(Duration::from_millis(10000)).await;
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(ApiResponse::<()>::error("TIMEOUT", "Simulated timeout")),
                ).into_response()
            }
            FailureType::ServiceUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiResponse::<()>::error(
                    "SERVICE_UNAVAILABLE",
                    "Simulated service unavailable",
                )),
            ).into_response(),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error("UNKNOWN", "Unknown failure type")),
            ).into_response(),
        };
    }

    // Latency injection
    latency_config.apply().await;

    // 2. Data retrieval and response determination
    let mode = params.mode.as_deref().unwrap_or("normal");
    let requested_page_size = params.page_size.unwrap_or(100).max(1);
    
    // Auto-switch to stream if count > 100,000 or mode is explicit stream
    if (requested_page_size as u64) > 100_000 || mode == "stream" {
        return handle_list_stream(state, params, requested_page_size as usize).await.into_response();
    }

    let (records, total_count, page_size, page, offset) = {
        let state = state.read().await;

        let page_size = (requested_page_size as u64).min(max_records as u64) as u32;

        tracing::info!(">>> [CORE-BANKING] Processing Credit List - page_size: {}", page_size);

        let (offset, page) = if params.cursor.is_some() {
            (parse_cursor(&params.cursor), 1u32)
        } else {
            let page = params.page.unwrap_or(1).max(1);
            (((page - 1) * page_size) as usize, page)
        };

        // Calculate total count (needed for metadata)
        let total_count = if cutoff_start.is_some() || cutoff_end.is_some() {
            state
                .records
                .values()
                .filter(|r| within_cutoff(&r.last_updated, cutoff_start, cutoff_end))
                .count() as u64
        } else {
            state.records.len() as u64
        };

        // Get page of records
        let records: Vec<CreditRecord> = if cutoff_start.is_some() || cutoff_end.is_some() {
            state
                .records
                .values()
                .filter(|r| within_cutoff(&r.last_updated, cutoff_start, cutoff_end))
                .skip(offset)
                .take(page_size as usize)
                .cloned()
                .collect()
        } else {
            state
                .records
                .values()
                .skip(offset)
                .take(page_size as usize)
                .cloned()
                .collect()
        };

        (records, total_count, page_size, page, offset)
    };

    let returned = records.len();

    let meta = ResponseMeta::paginated(page, page_size, total_count)
        .with_timing(start.elapsed().as_millis() as u64)
        .with_extra(
            "next_cursor",
            serde_json::json!(next_cursor_if_any(offset, returned, total_count as usize)),
        );

    // 3. Write lock only for updating stats
    {
        let mut state = state.write().await;
        state
            .stats
            .record_request("/api/v1/credits", true, start.elapsed().as_millis() as f64);

        // Approximate bytes sent to avoid expensive serialization
        let approx_bytes = (returned * 1024) as u64;
        state.stats.bytes_sent += approx_bytes;
    }

    (StatusCode::OK, Json(ApiResponse::success_with_meta(records, meta))).into_response()
}

/// Helper to handle streaming for GET /api/v1/credits
async fn handle_list_stream(
    state: SharedState<CoreBankingState>,
    params: ListCreditsParams,
    page_size: usize,
) -> Response {
    let chunk_size = params.stream_chunk_size.unwrap_or(1000).max(1);
    
    let cutoff_start = parse_date_opt(&params.cutoff_start).ok().flatten();
    let cutoff_end = parse_date_opt(&params.cutoff_end).ok().flatten();

    let stream = async_stream::stream! {
        let records: Vec<CreditRecord> = {
            let state_lock = state.read().await;
            
            let offset = if params.cursor.is_some() {
                parse_cursor(&params.cursor)
            } else {
                let page = params.page.unwrap_or(1).max(1);
                ((page - 1) as usize) * page_size
            };

            if cutoff_start.is_some() || cutoff_end.is_some() {
                state_lock.records.values()
                    .filter(|r| within_cutoff(&r.last_updated, cutoff_start, cutoff_end))
                    .skip(offset)
                    .take(page_size)
                    .cloned()
                    .collect()
            } else {
                state_lock.records.values()
                    .skip(offset)
                    .take(page_size)
                    .cloned()
                    .collect()
            }
        };

        let mut current_batch = String::new();
        let mut count = 0;

        for record in records {
            if let Ok(line) = serde_json::to_string(&record) {
                current_batch.push_str(&line);
                current_batch.push('\n');
                count += 1;

                if count >= chunk_size {
                    yield Ok::<_, Infallible>(current_batch.clone());
                    current_batch.clear();
                    count = 0;
                }
            }
        }

        if !current_batch.is_empty() {
            yield Ok::<_, Infallible>(current_batch);
        }
    };

    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Get single credit by ID
async fn get_credit_handler(
    State(state): State<SharedState<CoreBankingState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let state = state.read().await;
    if let Some(record) = state.records.get(&id) {
        (StatusCode::OK, Json(ApiResponse::success(record.clone())))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<CreditRecord>::error(
                "NOT_FOUND",
                "Credit record not found",
            )),
        )
    }
}

/// Get credit count (optionally filtered by cutoff window)
async fn count_credits_handler(
    State(state): State<SharedState<CoreBankingState>>,
    Query(params): Query<ListCreditsParams>,
) -> impl IntoResponse {
    let cutoff_start = match parse_date_opt(&params.cutoff_start) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("BAD_REQUEST", e))),
    };
    let cutoff_end = match parse_date_opt(&params.cutoff_end) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("BAD_REQUEST", e))),
    };

    let state = state.read().await;
    let count = if cutoff_start.is_some() || cutoff_end.is_some() {
        state
            .records
            .values()
            .filter(|r| within_cutoff(&r.last_updated, cutoff_start, cutoff_end))
            .count()
    } else {
        state.records.len()
    };

    (
        StatusCode::OK,
        Json(ApiResponse::success(serde_json::json!({
            "count": count,
            "dirty_ratio": state.dataset_dirty_ratio,
            "seed": state.dataset_seed
        }))),
    )
}

/// Generate new credits (ONLY mutating endpoint)
async fn generate_credits_handler(
    State(state): State<SharedState<CoreBankingState>>,
    Json(request): Json<GenerateCreditsRequest>,
) -> impl IntoResponse {
    let start = Instant::now();

    // 1. Read config and current state (lightweight read lock)
    let (max_records, default_dirty, current_count) = {
        let state = state.read().await;
        (
            state.config.max_records_per_request,
            state.config.default_dirty_ratio,
            state.records.len() as u64,
        )
    };

    let count = request.count.min(max_records);
    let dirty_ratio = request
        .dirty_ratio
        .unwrap_or(default_dirty)
        .clamp(0.0, 1.0);
    let seed = request.seed;
    let append = request.append.unwrap_or(false);
    let start_id = if append { current_count } else { 0 };

    // 2. Generate records in blocking thread (CPU heavy, no lock)
    let new_records = tokio::task::spawn_blocking(move || {
        CoreBankingState::generate_records(count, dirty_ratio, seed, start_id)
    })
    .await
    .unwrap();

    // 3. Update state (quick write lock)
    let mut state = state.write().await;
    if !append {
        state.records.clear();
    }
    // Pre-allocate if appending
    if append {
        state.records.reserve(count as usize);
    }

    for record in new_records {
        state.records.insert(record.id.clone(), record);
    }

    let result = serde_json::json!({
        "generated": count,
        "dirty_ratio": dirty_ratio,
        "seed": seed,
        "total_records": state.records.len()
    });

    (StatusCode::OK, Json(ApiResponse::success(result)))
}

/// Load specific credits (ONLY mutating endpoint)
async fn load_credits_handler(
    State(state): State<SharedState<CoreBankingState>>,
    Json(request): Json<LoadCreditsRequest>,
) -> impl IntoResponse {
    let append = request.append.unwrap_or(false);
    let count = request.records.len();

    // Update state (quick write lock)
    let mut state = state.write().await;
    if !append {
        state.records.clear();
    }
    // Pre-allocate if appending
    if append {
        state.records.reserve(count);
    }

    for record in request.records {
        state.records.insert(record.id.clone(), record);
    }

    let result = serde_json::json!({
        "loaded": count,
        "total_records": state.records.len()
    });

    (StatusCode::OK, Json(ApiResponse::success(result)))
}

/// Load specific credits from NDJSON stream (high-performance)
async fn load_ndjson_handler(
    State(state): State<SharedState<CoreBankingState>>,
    Query(params): Query<HashMap<String, String>>,
    body: Body,
) -> impl IntoResponse {
    let append = params.get("append").map(|v| v == "true").unwrap_or(false);
    
    let stream = body.into_data_stream();
    let stream = stream.map(|res| res.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)));
    let reader = StreamReader::new(stream);
    let mut lines = tokio::io::BufReader::new(reader).lines();
    
    let mut count = 0;
    let mut records_to_add = Vec::with_capacity(10000);
    
    if !append {
        let mut state_lock = state.write().await;
        state_lock.records.clear();
    }

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        if let Ok(record) = serde_json::from_str::<CreditRecord>(trimmed) {
            records_to_add.push(record);
            count += 1;
            
            if records_to_add.len() >= 10000 {
                let mut state_lock = state.write().await;
                for r in records_to_add.drain(..) {
                    state_lock.records.insert(r.id.clone(), r);
                }
            }
        }
    }
    
    if !records_to_add.is_empty() {
        let mut state_lock = state.write().await;
        for r in records_to_add {
            state_lock.records.insert(r.id.clone(), r);
        }
    }

    let total = {
        let state_lock = state.read().await;
        state_lock.records.len()
    };

    let result = serde_json::json!({
        "loaded": count,
        "total_records": total
    });

    (StatusCode::OK, Json(ApiResponse::success(result)))
}

/// Stream credits using Server-Sent Events (stateless generator)
async fn stream_credits_handler(
    State(state): State<SharedState<CoreBankingState>>,
    Query(params): Query<StreamCreditsParams>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let count = params.count.unwrap_or(1000);
    let dirty_ratio = params.dirty_ratio.unwrap_or(0.0).clamp(0.0, 1.0);
    let seed = params.seed;
    let batch_size = params.batch_size.unwrap_or(100).max(1);
    let delay_ms = params.delay_ms.unwrap_or(10);

    let stream = async_stream::stream! {
        let mut rng: ChaCha8Rng = match seed {
            Some(s) => ChaCha8Rng::seed_from_u64(s),
            None => ChaCha8Rng::from_entropy(),
        };

        let error_types = [
            "invalid_nik",
            "negative_amount",
            "invalid_date",
            "missing_field",
            "invalid_currency",
            "invalid_collectability",
            "outstanding_gt_plafon",
        ];

        let mut batch = Vec::with_capacity(batch_size as usize);
        let mut generated = 0u32;

        while generated < count {
            let mut record = CreditRecord::generate_clean(&mut rng, generated as u64);
            if dirty_ratio > 0.0 && rng.gen::<f64>() < dirty_ratio {
                let error_type = error_types[rng.gen_range(0..error_types.len())];
                record = record.inject_error(&mut rng, error_type);
            }

            batch.push(record);
            generated += 1;

            if batch.len() >= batch_size as usize || generated >= count {
                let data = serde_json::to_string(&batch).unwrap_or_default();
                yield Ok(Event::default().event("records").data(data));
                batch.clear();

                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }

        yield Ok(Event::default().event("complete").data(format!(r#"{{\"total\":{}}}"#, generated)));

        let mut state = state.write().await;
        state.stats.record_request("/api/v1/credits/stream", true, 0.0);
    };

    Sse::new(stream)
}

/// Stream credits in NDJSON (stateless generator)
async fn ndjson_credits_handler(
    State(state): State<SharedState<CoreBankingState>>,
    Query(params): Query<StreamCreditsParams>,
) -> Response {
    let count = params.count.unwrap_or(1000);
    let dirty_ratio = params.dirty_ratio.unwrap_or(0.0).clamp(0.0, 1.0);
    let seed = params.seed;
    let delay_ms = params.delay_ms.unwrap_or(0);

    let stream = async_stream::stream! {
        let mut rng: ChaCha8Rng = match seed {
            Some(s) => ChaCha8Rng::seed_from_u64(s),
            None => ChaCha8Rng::from_entropy(),
        };

        let error_types = [
            "invalid_nik",
            "negative_amount",
            "invalid_date",
            "missing_field",
            "invalid_currency",
            "invalid_collectability",
            "outstanding_gt_plafon",
        ];

        for i in 0..count {
            let mut record = CreditRecord::generate_clean(&mut rng, i as u64);
            if dirty_ratio > 0.0 && rng.gen::<f64>() < dirty_ratio {
                let error_type = error_types[rng.gen_range(0..error_types.len())];
                record = record.inject_error(&mut rng, error_type);
            }

            let line = match serde_json::to_string(&record) {
                Ok(s) => s,
                Err(_) => "{}".to_string(),
            };

            yield Ok::<_, Infallible>(format!("{}\n", line));

            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        // update stats (best effort)
        let mut st = state.write().await;
        st.stats.record_request("/api/v1/credits/ndjson", true, 0.0);
    };

    let body = Body::from_stream(stream);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/x-ndjson".parse().unwrap());

    (StatusCode::OK, headers, body).into_response()
}

/// Get simulator statistics
async fn stats_handler(State(state): State<SharedState<CoreBankingState>>) -> impl IntoResponse {
    let state = state.read().await;
    Json(ApiResponse::success(state.stats.clone()))
}

/// Reset simulator state
async fn reset_handler(State(state): State<SharedState<CoreBankingState>>) -> impl IntoResponse {
    let mut state = state.write().await;
    state.stats = SimulatorStats::default();
    let record_count = state.config.default_record_count;
    let dirty_ratio = state.config.default_dirty_ratio;
    let seed = state.config.seed;

    let records = CoreBankingState::generate_records(record_count, dirty_ratio, seed, 0);
    state.records.clear();
    for record in records {
        state.records.insert(record.id.clone(), record);
    }
    state.dataset_seed = seed;
    state.dataset_dirty_ratio = dirty_ratio;

    Json(ApiResponse::success(serde_json::json!({
        "reset": true,
        "record_count": state.records.len(),
        "dirty_ratio": state.dataset_dirty_ratio,
        "seed": state.dataset_seed
    })))
}

/// Update configuration dynamically
async fn update_config_handler(
    State(state): State<SharedState<CoreBankingState>>,
    Json(request): Json<UpdateConfigRequest>,
) -> impl IntoResponse {
    let mut state = state.write().await;

    if let Some(dirty_ratio) = request.dirty_ratio {
        state.config.default_dirty_ratio = dirty_ratio.clamp(0.0, 1.0);
    }

    if let Some(latency_ms) = request.latency_ms {
        state.config.latency = LatencyConfig {
            enabled: latency_ms > 0,
            base_ms: latency_ms,
            jitter_ms: latency_ms / 10,
            percentiles: None,
        };
    }

    if let Some(failure_rate) = request.failure_rate {
        state.config.failure_injection.enabled = failure_rate > 0.0;
        state.config.failure_injection.failure_rate = failure_rate.clamp(0.0, 1.0);
    }

    Json(ApiResponse::success(serde_json::json!({
        "updated": true,
        "dirty_ratio": state.config.default_dirty_ratio,
        "latency_enabled": state.config.latency.enabled,
        "failure_rate": state.config.failure_injection.failure_rate
    })))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credit_record_generate_clean() {
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let record = CreditRecord::generate_clean(&mut rng, 1);

        assert_eq!(record.id, "CR0000000001");
        assert_eq!(record.nik.len(), 16);
        assert!(record.nik.chars().all(|c| c.is_numeric()));
        assert!(!record.nama_lengkap.is_empty());
        assert_eq!(record.mata_uang, "IDR");
        assert!(record.jumlah_kredit > 0);
        assert!(record.saldo_outstanding <= record.jumlah_kredit);
        assert!((1..=5).contains(&record.kolektabilitas));
        assert!(record._has_error.is_none());
    }

    #[test]
    fn test_credit_record_inject_invalid_nik() {
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let record = CreditRecord::generate_clean(&mut rng, 1).inject_error(&mut rng, "invalid_nik");

        assert_eq!(record._has_error, Some(true));
        assert_eq!(record._error_type, Some("invalid_nik".to_string()));
        assert!(record.nik.len() != 16 || !record.nik.chars().all(|c| c.is_numeric()));
    }

    #[test]
    fn test_credit_record_inject_negative_amount() {
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let record = CreditRecord::generate_clean(&mut rng, 1).inject_error(&mut rng, "negative_amount");

        assert_eq!(record._has_error, Some(true));
        assert!(record.jumlah_kredit < 0);
    }

    #[test]
    fn test_credit_record_inject_invalid_date() {
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let record = CreditRecord::generate_clean(&mut rng, 1).inject_error(&mut rng, "invalid_date");

        assert_eq!(record._has_error, Some(true));
        let invalid_start = record.tanggal_mulai == "2024-13-45" || record.tanggal_mulai == "not-a-date";
        let invalid_end = record.tanggal_jatuh_tempo.is_empty();
        assert!(invalid_start || invalid_end);
    }

    #[test]
    fn test_state_generate_records_deterministic() {
        let config = CoreBankingConfig::default();

        let mut state1 = CoreBankingState::new(config.clone());
        let records1 = CoreBankingState::generate_records(100, 0.1, Some(12345), 0);
        for r in records1 {
            state1.records.insert(r.id.clone(), r);
        }

        let mut state2 = CoreBankingState::new(config);
        let records2 = CoreBankingState::generate_records(100, 0.1, Some(12345), 0);
        for r in records2 {
            state2.records.insert(r.id.clone(), r);
        }

        assert_eq!(state1.records.len(), state2.records.len());
        for (id, r1) in &state1.records {
            let r2 = state2.records.get(id).expect("Record missing in state2");
            assert_eq!(r1.id, r2.id);
            assert_eq!(r1.nik, r2.nik);
            assert_eq!(r1.jumlah_kredit, r2.jumlah_kredit);
            assert_eq!(r1._has_error, r2._has_error);
        }
    }

    #[test]
    fn test_within_cutoff() {
        let ts = "2024-01-15T10:00:00Z";
        assert!(within_cutoff(ts, Some(NaiveDate::from_ymd_opt(2024,1,1).unwrap()), Some(NaiveDate::from_ymd_opt(2024,1,31).unwrap())));
        assert!(!within_cutoff(ts, Some(NaiveDate::from_ymd_opt(2024,2,1).unwrap()), None));
    }
}
