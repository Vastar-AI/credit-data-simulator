# Credit Data Simulator — Performance Report

Built for benchmarking [VIL (Vastar Intermediate Language)](https://github.com/OceanOS-id/VIL) NDJSON pipeline examples. These numbers establish the **simulator baseline** — any overhead measured when running VIL pipeline examples against this simulator is VIL's own processing cost, not the simulator's.

**System:** Intel i9-11900F @ 2.50GHz (8C/16T), 32GB RAM, Ubuntu 22.04 (kernel 6.8.0)
**Rust:** 1.93.1 | **Framework:** axum 0.7 | **Build:** `--release`
**Tool:** `oha` | **Date:** 2026-03-27
**Note:** Machine under normal development workload (load avg ~1.3) during all benchmarks.

---

## Per-Endpoint Results (c200, n2000)

| Endpoint | Description | req/s | P50 | P99 | Success |
|----------|-------------|-------|-----|-----|---------|
| `GET /health` | Health check | **52,744** | 0.3ms | 27ms | 100% |
| `GET /api/v1/credits?page_size=100` | JSON paginated (100 rec) | **10,591** | 16ms | 24ms | 100% |
| `GET /api/v1/credits/ndjson?page_size=100` | NDJSON stream (100 rec) | **1,910** | 96ms | 202ms | 100% |
| `GET /api/v1/credits/ndjson?page_size=1000` | NDJSON stream (1000 rec) | **1,935** | 97ms | 190ms | 100% |
| `GET /api/v1/mappings/v1/fields` | Mapping fields | **62,347** | 0.5ms | 23ms | 100% |
| `GET /api/v1/rulepacks/v1/rules` | Validation rules | **73,154** | 0.3ms | 21ms | 100% |

---

## Scaling — NDJSON (100 records/page)

| Concurrent | Total Req | req/s | P50 | P99 | P99.9 | Slowest | Success |
|-----------|-----------|-------|-----|-----|-------|---------|---------|
| 200 | 2,000 | 1,910 | 96ms | 202ms | 216ms | 216ms | 100% |
| 500 | 5,000 | 1,963 | 236ms | 535ms | 1.0s | 1.0s | 100% |
| 1,000 | 10,000 | 2,004 | 459ms | 2.1s | 2.3s | 2.3s | 100% |

## Scaling — JSON Paginated (100 records/page)

| Concurrent | Total Req | req/s | P50 | P99 | P99.9 | Slowest | Success |
|-----------|-----------|-------|-----|-----|-------|---------|---------|
| 200 | 2,000 | 10,591 | 16ms | 24ms | 35ms | 35ms | 100% |
| 500 | 5,000 | 10,500 | 43ms | 64ms | 80ms | 85ms | 100% |
| 1,000 | 10,000 | 10,776 | 87ms | 98ms | 109ms | 121ms | 100% |

---

## Analysis

### NDJSON Streaming

NDJSON throughput is ~2,000 req/s regardless of concurrency. Each response streams 100 serialized credit records — the bottleneck is serialization + TCP write, not connection handling. At 100 records/request, this translates to **~200K records/sec** raw throughput.

**VIL comparison:** VIL NDJSON pipeline examples (005, 007-009) measure ~800-1,100 req/s through the full pipeline (source → transform → sink). The ~50% overhead is VIL's business logic (parse, enrich/filter/validate, re-serialize), not the simulator.

### JSON Paginated

JSON paginated endpoint sustains **~10,500 req/s** — the entire 100-record page is pre-serialized as a single JSON array. This is 5x faster than NDJSON because there's no per-line streaming overhead.

### Metadata Services

Mapping (62K req/s) and Rulepack (73K req/s) services are essentially in-memory lookups with tiny payloads — they will never be the bottleneck.

---

## Reproduction

```bash
# Start simulator
./run_simulator.sh

# NDJSON benchmark
oha -c 200 -n 2000 'http://localhost:18081/api/v1/credits/ndjson?page_size=100'

# JSON benchmark
oha -c 200 -n 2000 'http://localhost:18081/api/v1/credits?page_size=100'

# Mapping service
oha -c 200 -n 2000 'http://localhost:18082/api/v1/mappings/v1/fields'
```
