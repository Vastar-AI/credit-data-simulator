# Credit Data Simulator

Multi-service credit data pipeline simulator built with Rust and axum. Provides realistic Indonesian banking credit data with NDJSON bulk export, field mapping, validation rules, and regulatory submission endpoints.

Built primarily for benchmarking [VIL (Vastar Intermediate Language)](https://github.com/OceanOS-id/VIL) NDJSON pipeline examples (005, 007-009, 101-107). These numbers establish the simulator baseline — any overhead measured through VIL is VIL's own processing cost. Also usable as a standalone mock for any credit/banking data integration testing.

## Services

| Service | Port | Description |
|---------|------|-------------|
| **Core Banking** | 18081 | Credit records — NDJSON stream, JSON paginated, SSE, filtering, dirty data |
| **Mapping Service** | 18082 | SLIK field mapping versions (v1/v2/v3) |
| **Rulepack Service** | 18083 | Validation rules engine (NIK, amounts, dates, cross-field) |
| **Regulator Endpoint** | 18084 | OJK submission simulator (accept/reject/delay modes) |

## Quick Start

```bash
git clone https://github.com/Vastar-AI/credit-data-simulator.git
cd credit-data-simulator
cargo build --release
./run_simulator.sh
```

The `run_simulator.sh` script auto-kills any processes on ports 18081-18084 before starting.

## Endpoints

### Core Banking (:18081)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check |
| GET | `/api/v1/credits?page=1&page_size=100` | Paginated JSON (10,500 req/s) |
| GET | `/api/v1/credits/ndjson?page_size=100` | NDJSON bulk stream (2,000 req/s) |
| GET | `/api/v1/credits/stream` | SSE streaming |
| GET | `/api/v1/credits/count` | Total record count |
| GET | `/api/v1/credits/:id` | Single record by ID |
| POST | `/api/v1/credits/generate` | Generate new batch |
| GET | `/api/v1/stats` | Simulator statistics |
| POST | `/api/v1/reset` | Reset state |

**Query Parameters:**
- `page` / `page_size` — Pagination (default: 1, 100)
- `cursor` — Cursor-based pagination (optimized)
- `cutoff_start` / `cutoff_end` — Date range filter (YYYY-MM-DD)

### Mapping Service (:18082)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/mappings` | List mapping versions |
| GET | `/api/v1/mappings/:version` | Get mapping by version |
| GET | `/api/v1/mappings/:version/fields` | Get field mappings |
| POST | `/api/v1/mappings` | Create custom mapping |

**Pre-loaded:** v1 (SLIK-2023), v2 (SLIK-2024), v3 (SLIK-2024-REG)

### Rulepack Service (:18083)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/rulepacks` | List rulepack versions |
| GET | `/api/v1/rulepacks/:version/rules` | Get rules |
| POST | `/api/v1/rulepacks/:version/validate` | Validate data against rules |

**Rule types:** required, length, range, pattern, enum, date_format, cross-field

### Regulator Endpoint (:18084)

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/v1/submit` | Submit credit data |
| GET | `/api/v1/submissions` | List submissions |
| GET | `/api/v1/submissions/:id` | Submission status |
| POST | `/api/v1/mode` | Change response mode (Accept/Reject/Delay) |

## Performance

**System:** Intel i9-11900F (8C/16T), 32GB RAM, Ubuntu 22.04, Rust 1.93.1

| Endpoint | req/s | records/s | P50 | P99 | Notes |
|----------|-------|-----------|-----|-----|-------|
| Health check | 52,744 | — | 0.3ms | 27ms | Baseline |
| JSON 100 rec/page | 10,591 | **1,059K** | 16ms | 24ms | Pre-serialized array |
| NDJSON 100 rec/page | 1,910 | **191K** | 96ms | 202ms | Per-line streaming |
| NDJSON 1000 rec/page | 1,935 | **1,935K** | 97ms | 190ms | 10x records, same req/s |
| Mapping fields | 62,347 | — | 0.5ms | 23ms | In-memory lookup |
| Rulepack rules | 73,154 | — | 0.3ms | 21ms | In-memory lookup |

**NDJSON throughput: ~191K records/sec** (100 rec/page) or **~1.9M records/sec** (1000 rec/page).

Full benchmark: [PERFORMANCE_REPORT.md](./PERFORMANCE_REPORT.md)

## Data Model

Each credit record contains realistic Indonesian banking data:

```json
{
  "id": "CR0000000000",
  "nik": "8272346211567351",
  "nama_lengkap": "Queena Simanjuntak",
  "jenis_fasilitas": "KKB",
  "jumlah_kredit": 8804025599,
  "mata_uang": "IDR",
  "suku_bunga_bps": 579,
  "tanggal_mulai": "2023-06-05",
  "tanggal_jatuh_tempo": "2026-06-16",
  "saldo_outstanding": 6933193728,
  "kolektabilitas": 3,
  "kode_cabang": "BDG001",
  "account_officer": "AO0835",
  "last_updated": "2024-01-01T10:00:00Z"
}
```

**Dirty data injection:** Configurable ratio (0-100%) with error types: invalid NIK, negative amounts, invalid dates, missing fields, invalid currency, invalid collectability, outstanding > plafon.

## Data Generator (CLI)

Generate NDJSON files offline:

```bash
# 100K records, 25% dirty
./target/release/datagen -c 100000 -d 0.25 -o credits.ndjson

# Load into running simulator
./target/release/datagen -c 50000 -d 0.1 --load-to http://localhost:18081
```

## Testing

```bash
# Health
curl http://localhost:18081/health

# NDJSON stream (first 3 records)
curl 'http://localhost:18081/api/v1/credits/ndjson?page_size=3'

# JSON paginated
curl 'http://localhost:18081/api/v1/credits?page=1&page_size=10'

# Count
curl http://localhost:18081/api/v1/credits/count

# Mapping fields
curl http://localhost:18082/api/v1/mappings/v1/fields

# Validation rules
curl http://localhost:18083/api/v1/rulepacks/v1/rules

# Benchmark
oha -c 200 -n 2000 'http://localhost:18081/api/v1/credits/ndjson?page_size=100'
```

## Project Structure

```
credit-data-simulator/
├── src/
│   ├── main.rs                 # Entry point — starts all 4 services
│   ├── lib.rs                  # SimulatorServer, common types, traits
│   ├── config.rs               # SimulatorConfig with per-service settings
│   ├── core_banking.rs         # Credit data: NDJSON, JSON, SSE, filtering
│   ├── mapping_service.rs      # SLIK field mapping versions
│   ├── rulepack_service.rs     # Validation rules engine
│   ├── regulator_endpoint.rs   # OJK submission simulator
│   ├── engine.rs               # Engine + Admin simulators
│   ├── models/                 # Credit record model + data generation
│   └── bin/datagen.rs          # CLI data generator
├── run_simulator.sh            # Start script (auto port cleanup)
├── stop_simulator.sh           # Stop script
├── PERFORMANCE_REPORT.md       # Benchmark results
├── Cargo.toml
└── .gitignore
```

## License

MIT OR Apache-2.0

## Links

- [VIL](https://github.com/OceanOS-id/VIL) — Process-oriented zero-copy framework
- [AI Endpoint Simulator](https://github.com/Vastar-AI/ai-endpoint-simulator) — Multi-dialect SSE simulator
