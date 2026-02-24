# Build stage
FROM rust:1.75 AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y \
    libssl-dev \
    pkg-config \
    clang \
    lld \
    libssh2-1-dev \
    && rm -rf /var/lib/apt/lists/*

COPY . .

RUN cargo build --release --locked

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    libssh2-1 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/dload /usr/local/bin/

RUN mkdir /data && chown nobody /data

USER nobody

EXPOSE 8080

CMD ["dload"]
