# syntax=docker/dockerfile:1

FROM rust:1.86-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libasound2-dev \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests first for layer caching
COPY Cargo.toml Cargo.lock ./

# Create dummy src to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "pub fn dummy() {}" > src/lib.rs
RUN cargo build --release || true
RUN rm -rf src target/release/deps/beacon* target/release/.fingerprint/beacon*

# Copy source and personas (needed at compile time for include_str!)
COPY src ./src
COPY personas ./personas
RUN touch src/lib.rs && cargo build --release

# Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libasound2 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/beacon /usr/local/bin/beacon

# Copy personas (must be in build context)
COPY personas /etc/beacon/personas

ENV RUST_LOG=info
ENV BEACON_PERSONAS_DIR=/etc/beacon/personas

EXPOSE 18789 18790

CMD ["beacon"]
