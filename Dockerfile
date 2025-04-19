FROM rust:1.84.0 AS chef
RUN cargo install cargo-chef --version 0.1.67
WORKDIR /app

FROM chef AS planner
COPY . .
RUN sed -i 's/openapi = { path = "\.\/client" }//g' Cargo.toml # cargo cheff doesn't like the local dev-dependency
RUN cargo chef prepare  --recipe-path recipe.json

FROM chef AS builder
# ENV DATABASE_URL=sqlite:src.db
# FIXME: libssl-dev is only actually required for x86, skip for arm64
RUN apt-get update && apt-get install -y cmake libssl-dev
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

# # FROM alpine:3.20.3
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates # TODO: try to copy over certs from previous stage instead of running apt-get update
COPY --from=builder /app/target/release/main /usr/local/bin/prezel
CMD ["prezel"]
