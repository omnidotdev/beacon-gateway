# syntax=docker/dockerfile:1

FROM rust:1.88-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libasound2-dev \
    curl \
    git \
    && rm -rf /var/lib/apt/lists/*

# Clone agent-core (path dep resolves to /agent-core from WORKDIR /app)
ARG GITHUB_TOKEN
RUN if [ -n "$GITHUB_TOKEN" ]; then \
      git clone --depth 1 https://x-access-token:${GITHUB_TOKEN}@github.com/omnidotdev/agent-core.git /agent-core; \
    else \
      git clone --depth 1 https://github.com/omnidotdev/agent-core.git /agent-core; \
    fi

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
ENV BEACON_DISABLE_VOICE=true

EXPOSE 18789 18790

CMD ["beacon"]
