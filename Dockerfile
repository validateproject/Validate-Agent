FROM rust:1.84-bookworm AS builder
WORKDIR /app

# Install build dependencies for crates that need OpenSSL.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates \
        protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.toml
COPY crates crates

RUN cargo fetch

COPY . .

RUN cargo build --release --workspace

FROM debian:bookworm-slim AS runtime
ARG APP_USER=validator
ENV RUST_LOG=info \
    APP_HOME=/app

RUN useradd --create-home --home-dir "${APP_HOME}" --shell /bin/bash "${APP_USER}" \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR ${APP_HOME}

COPY --from=builder /app/target/release/agent /usr/local/bin/agent
COPY --from=builder /app/target/release/metrics_collector /usr/local/bin/metrics_collector
COPY --from=builder /app/target/release/executor_daemon /usr/local/bin/executor_daemon
COPY --from=builder /app/target/release/validator_client /usr/local/bin/validator_client

RUN chown -R "${APP_USER}:${APP_USER}" "${APP_HOME}"

USER ${APP_USER}

CMD ["/usr/local/bin/agent"]

