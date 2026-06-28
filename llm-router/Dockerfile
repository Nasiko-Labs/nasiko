FROM rust:1.91-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY oss/ oss/

# Remove EE members from the workspace so cargo doesn't require ee/ to exist
RUN sed -i '/"ee\//d' Cargo.toml

RUN cargo build --release -p nasiko-llm-router --bin llm-router

FROM debian:trixie-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/llm-router .

# NOTE: no migrations are copied — nasiko-server owns the schema. Point this at an
# already-migrated Postgres (DATABASE_URL). See src/bin/llm-router.rs.
EXPOSE 8081

CMD ["./llm-router"]
