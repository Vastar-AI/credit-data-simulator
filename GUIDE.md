# Credit Data Simulator — User Guide

This guide covers data generation, loading fixed datasets for reproducible benchmarks, and configuring the simulator for different testing scenarios.

## Table of Contents

1. [Quick Start](#quick-start)
2. [Data Generation](#data-generation)
3. [Loading Fixed Datasets](#loading-fixed-datasets)
4. [Reproducible Benchmarks](#reproducible-benchmarks)
5. [Dirty Data Testing](#dirty-data-testing)
6. [API Workflows](#api-workflows)
7. [Configuration](#configuration)
8. [Environment Variables](#environment-variables)

---

## Quick Start

```bash
# Build
cargo build --release

# Start (auto-cleans ports 18081-18084)
./run_simulator.sh

# Verify
curl http://localhost:18081/health
curl http://localhost:18081/api/v1/credits/count
```

On startup the simulator auto-generates **100,000 clean credit records** in memory. No files needed.

---

## Data Generation

The `datagen` CLI tool generates NDJSON files with configurable record count, dirty ratio, and deterministic seed.

### Generate Clean Data

```bash
# 100 records, clean (no errors)
./target/release/datagen -c 100 -o clean_100.ndjson

# 100K records, clean
./target/release/datagen -c 100000 -o clean_100k.ndjson

# 1M records, clean
./target/release/datagen -c 1000000 -o clean_1m.ndjson
```

### Generate Dirty Data (Error Injection)

The `-d` flag sets the ratio of records with injected errors (0.0 = clean, 1.0 = all dirty):

```bash
# 100K records, 25% dirty
./target/release/datagen -c 100000 -d 0.25 -o dirty_25pct_100k.ndjson

# 100K records, 10% dirty
./target/release/datagen -c 100000 -d 0.10 -o dirty_10pct_100k.ndjson

# 100K records, 100% dirty (stress test)
./target/release/datagen -c 100000 -d 1.0 -o all_dirty_100k.ndjson
```

**Error types injected:**

| Error Type | Description | Example |
|-----------|-------------|---------|
| `invalid_nik` | NIK not 16 digits | "123" or "99999999999999999" |
| `negative_amount` | Negative loan amount | -500000 |
| `invalid_date` | Impossible date | "2099-13-45" |
| `missing_field` | Empty required field | `nama_lengkap: ""` |
| `invalid_currency` | Non-standard currency | "XYZ" instead of "IDR" |
| `invalid_collectability` | Out-of-range value | 0 or 6 (valid: 1-5) |
| `outstanding_gt_plafon` | Outstanding exceeds loan amount | saldo > jumlah_kredit |

### Deterministic Generation (Seed)

Use `-s` for reproducible datasets. Same seed always produces identical records:

```bash
# Generate with seed — always the same output
./target/release/datagen -c 100000 -d 0.25 -s 42 -o benchmark_dataset.ndjson

# Verify reproducibility
./target/release/datagen -c 100000 -d 0.25 -s 42 -o benchmark_dataset_2.ndjson
diff benchmark_dataset.ndjson benchmark_dataset_2.ndjson
# No output = identical files
```

This is critical for **consistent benchmarking** — the same dataset means the same serialization overhead, same dirty ratio, same record sizes every run.

---

## Loading Fixed Datasets

By default the simulator generates random data on startup. For **reproducible benchmarks**, generate a fixed dataset once and load it into the simulator.

### Step 1: Generate the Benchmark Dataset

```bash
# Standard benchmark dataset: 100K records, 25% dirty, seed 42
./target/release/datagen -c 100000 -d 0.25 -s 42 -o benchmark_100k.ndjson
```

### Step 2: Start Simulator

```bash
./run_simulator.sh
```

### Step 3: Load the Fixed Dataset (Replace)

```bash
# Replace all in-memory records with the fixed dataset
./target/release/datagen -f benchmark_100k.ndjson --load-to http://localhost:18081
```

This uploads the NDJSON file via `POST /api/v1/credits/load-ndjson`. The simulator replaces its in-memory dataset entirely.

### Step 4: Verify

```bash
curl http://localhost:18081/api/v1/credits/count
# {"success":true,"data":{"count":100000,"dirty_ratio":0.25,"seed":42}}
```

### Append Mode

To add records to the existing dataset (instead of replacing):

```bash
# Append 50K more records
./target/release/datagen -f extra_50k.ndjson --load-to http://localhost:18081 --append
```

### Direct Generate + Load (One Command)

Generate and load in one step:

```bash
# Generate 100K records and immediately load into running simulator
./target/release/datagen -c 100000 -d 0.25 -s 42 --load-to http://localhost:18081
```

---

## Reproducible Benchmarks

For consistent benchmark results, always use the same dataset:

### Create Standard Benchmark Datasets

```bash
# Small (dev/CI)
./target/release/datagen -c 10000 -d 0.25 -s 42 -o bench_10k.ndjson

# Medium (integration)
./target/release/datagen -c 100000 -d 0.25 -s 42 -o bench_100k.ndjson

# Large (load test)
./target/release/datagen -c 1000000 -d 0.25 -s 42 -o bench_1m.ndjson
```

### Run Benchmark with Fixed Data

```bash
# 1. Start simulator
./run_simulator.sh

# 2. Load fixed dataset
./target/release/datagen -f bench_100k.ndjson --load-to http://localhost:18081

# 3. Run benchmark (always same data)
oha -c 200 -n 2000 'http://localhost:18081/api/v1/credits/ndjson?page_size=100'

# 4. Reset and repeat (same results)
curl -X POST http://localhost:18081/api/v1/reset
./target/release/datagen -f bench_100k.ndjson --load-to http://localhost:18081
oha -c 200 -n 2000 'http://localhost:18081/api/v1/credits/ndjson?page_size=100'
```

### VIL Pipeline Benchmark Workflow

When benchmarking VIL NDJSON pipeline examples (005, 007-009):

```bash
# 1. Start simulator with fixed data
./run_simulator.sh
./target/release/datagen -f bench_100k.ndjson --load-to http://localhost:18081

# 2. Run VIL example
cd /path/to/vil
cargo run -p basic-multiservice-mesh-ndjson --release

# 3. Benchmark VIL pipeline (VIL overhead = VIL result - simulator baseline)
oha -c 200 -n 2000 'http://localhost:3080/trigger'

# 4. Compare with simulator baseline (same dataset)
oha -c 200 -n 2000 'http://localhost:18081/api/v1/credits/ndjson?page_size=100'
```

---

## Dirty Data Testing

Dirty data ratio controls the percentage of records with injected errors. This is essential for testing VIL validation pipelines (008-quality-monitor).

### Generate Runtime (API)

```bash
# Generate 10K records with 50% dirty ratio via API
curl -X POST 'http://localhost:18081/api/v1/credits/generate?count=10000&dirty_ratio=0.5'
```

### Pre-generated File

```bash
# 100K records, 50% dirty
./target/release/datagen -c 100000 -d 0.5 -s 42 -o high_dirty_100k.ndjson
./target/release/datagen -f high_dirty_100k.ndjson --load-to http://localhost:18081
```

### Dirty Ratio Guide

| Ratio | Use Case | Example |
|-------|----------|---------|
| `0.0` | Clean data, throughput benchmark | 005-multiservice-mesh |
| `0.10` | Light validation, realistic prod | 007-npl-filter |
| `0.25` | Standard benchmark (recommended) | 008-quality-monitor |
| `0.50` | Stress test validation rules | Quality assurance |
| `1.0` | All records have errors | Negative testing |

---

## API Workflows

### Pagination

```bash
# Page 1 (100 records)
curl 'http://localhost:18081/api/v1/credits?page=1&page_size=100'

# Page 2
curl 'http://localhost:18081/api/v1/credits?page=2&page_size=100'

# Cursor-based (faster for large datasets)
curl 'http://localhost:18081/api/v1/credits?cursor=CR0000000100&page_size=100'
```

### Date Range Filtering

```bash
# Records updated in January 2024
curl 'http://localhost:18081/api/v1/credits?cutoff_start=2024-01-01&cutoff_end=2024-01-31&page_size=100'
```

### NDJSON Streaming

```bash
# Stream 1000 records as NDJSON
curl 'http://localhost:18081/api/v1/credits/ndjson?page_size=1000'

# Pipe to file
curl -s 'http://localhost:18081/api/v1/credits/ndjson?page_size=100000' > export.ndjson
```

### Field Mapping Lookup

```bash
# Get v2 mapping fields
curl http://localhost:18082/api/v1/mappings/v2/fields

# List all versions
curl http://localhost:18082/api/v1/mappings
```

### Validation

```bash
# Validate a record against rulepack v1
curl -X POST http://localhost:18083/api/v1/rulepacks/v1/validate \
  -H 'Content-Type: application/json' \
  -d '{"nik":"1234567890123456","jumlah_kredit":1000000,"mata_uang":"IDR"}'
```

### Regulator Submission

```bash
# Submit credit data
curl -X POST http://localhost:18084/api/v1/submit \
  -H 'Content-Type: application/json' \
  -d '{"records":[{"id":"CR001","nik":"1234567890123456"}]}'

# Check submission status
curl http://localhost:18084/api/v1/submissions

# Change mode (Accept/Reject/Delay)
curl -X POST http://localhost:18084/api/v1/mode \
  -H 'Content-Type: application/json' \
  -d '{"mode":"Reject"}'
```

---

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CORE_BANKING_PORT` | 18081 | Core Banking service port |
| `MAPPING_SERVICE_PORT` | 18082 | Mapping Service port |
| `RULEPACK_SERVICE_PORT` | 18083 | Rulepack Service port |
| `REGULATOR_ENDPOINT_PORT` | 18084 | Regulator Endpoint port |
| `RUST_LOG` | info | Log level (trace/debug/info/warn/error) |

### Programmatic Configuration

```rust
use credit_data_simulator::{SimulatorServer, SimulatorConfig, CoreBankingConfig};

let config = SimulatorConfig {
    core_banking: CoreBankingConfig {
        port: 19081,
        default_record_count: 50_000,
        default_dirty_ratio: 0.25,
        seed: Some(42),
        ..Default::default()
    },
    ..Default::default()
};

let server = SimulatorServer::start(config).await?;
```

### Preset Configurations

```rust
// CI — no latency, fast
let config = SimulatorConfig::for_ci();

// Load test — realistic latencies
let config = SimulatorConfig::for_load_test();

// Custom ports
let server = start_simulators_on_ports(19081, 19082, 19083, 19084).await?;
```

---

## datagen CLI Reference

```
Usage: datagen [OPTIONS]

Options:
  -c, --count <COUNT>          Number of records to generate [default: 100]
  -o, --output <OUTPUT>        Output file path (NDJSON)
  -d, --dirty-ratio <RATIO>    Error injection ratio 0.0-1.0 [default: 0.0]
  -s, --seed <SEED>            Random seed for deterministic generation
  -f, --file <FILE>            Load existing NDJSON file (skip generation)
  -l, --load-to <URL>          Upload to running simulator
  -a, --append                 Append to existing records (default: replace)
  -h, --help                   Print help
  -V, --version                Print version
```

### Examples

```bash
# Generate 100K clean records
datagen -c 100000 -o clean.ndjson

# Generate 500K with 25% dirty, seed 42
datagen -c 500000 -d 0.25 -s 42 -o benchmark.ndjson

# Load file into simulator (replace)
datagen -f benchmark.ndjson --load-to http://localhost:18081

# Load file into simulator (append)
datagen -f extra.ndjson --load-to http://localhost:18081 --append

# Generate and load in one step
datagen -c 100000 -d 0.25 -s 42 --load-to http://localhost:18081
```
