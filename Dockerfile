FROM lukemathwalker/cargo-chef:latest-rust-1.99-alpine3.24 AS chef
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release --locked

FROM alpine:3.24

RUN apk add --no-cache ca-certificates su-exec \
    && mkdir -p /data /downloads

COPY --from=builder /app/target/release/dload /usr/local/bin/
COPY --chmod=755 docker-entrypoint.sh /usr/local/bin/

EXPOSE 8080

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["dload"]
