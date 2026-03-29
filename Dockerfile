# =============================================================================
# Credit Data Simulator — Multi-stage build (static musl binary)
# =============================================================================

FROM rust:1.93-slim AS builder
WORKDIR /app

RUN apt-get update && apt-get install -y \
    build-essential pkg-config musl-tools \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-unknown-linux-musl

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN CC=musl-gcc cargo build --release --target x86_64-unknown-linux-musl

# Runtime — Alpine (tiny)
FROM alpine:3.19
WORKDIR /app

RUN apk add --no-cache ca-certificates

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/credit-data-simulator ./

EXPOSE 18081
CMD ["./credit-data-simulator"]
