# Build stage
FROM rust:1.93-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependencies - build a dummy project first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release --locked
# Remove only the dummy source, keep cached deps in target/
RUN rm -rf src

# Build the real project
COPY . .
RUN touch src/main.rs && cargo build --release --locked

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for security
RUN groupadd --system --gid 1000 dload && \
    useradd --system --uid 1000 --gid 1000 --home /app dload

WORKDIR /app

COPY --from=builder /app/target/release/dload /usr/local/bin/

RUN mkdir -p /data /downloads && chown -R dload:dload /data /downloads

USER dload:dload

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=3s --start-period=20s --retries=3 \
    CMD wget -qO- http://127.0.0.1:8080/healthz || exit 1

CMD ["dload"]
