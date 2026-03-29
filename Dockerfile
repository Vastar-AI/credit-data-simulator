# =============================================================================
# Credit Data Simulator — Multi-stage build
# =============================================================================

FROM rust:1.93-slim AS builder
WORKDIR /app

RUN apt-get update && apt-get install -y \
    build-essential pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build for native target (not musl — avoids OpenSSL cross-compile issues)
RUN cargo build --release

# Runtime — Debian slim (small, has glibc + SSL)
FROM debian:bookworm-slim
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/credit-data-simulator ./

EXPOSE 18081
CMD ["./credit-data-simulator"]
