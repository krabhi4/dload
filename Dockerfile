# Build stage
FROM rust:1.96-slim AS builder

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

# gosu: entrypoint uses it to drop to PUID/PGID.
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    gosu \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/dload /usr/local/bin/
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

RUN mkdir -p /data /downloads

EXPOSE 8080

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["dload"]
