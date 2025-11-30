# Validate Agent

Validate Agent is a automates validator SRE. It continuously monitors validator metrics, detects unhealthy conditions and try to fix the problem to prevent down time.

## What’s in the box?

| Component | Path | Description |
| --- | --- | --- |
| Metrics Collector | `crates/metrics_collector` | Subscribes to the daemon’s metrics stream and mirrors the latest samples into Redis for the dashboard. |
| Agent | `crates/agent` | Subscribes to validator metrics via gRPC, detects issues, sends actions through the daemon, and exposes the HTTP API for the dashboard. |
| Executor | `crates/executor` | gRPC control plane (`executor_daemon`), validator-side client (`validator_client`), and shared proto definitions. |
| Shared types | `crates/common` | Validator config, metrics schema, action definitions, and helper utilities. |
| Dashboard | `dashboard/` | A simple dashboard that consumes the agent API and visualizes validators, risk scores, and pending actions. |
| Docker mocks | `docker/validator-mock` | Python service that exposes `/metrics` plus `/admin/*` control hooks the executor calls. |

Redis now only stores the latest validator metrics (`validator:metrics:<id>`), mirrored there by the metrics collector for the dashboard; all action dispatching flows through the gRPC control plane.

## Agentic remediation (optional)

The agent can now call out to OpenAI to synthesize remediation plans dynamically. Enable it by adding an `agentic` block to `config.toml` (or providing the equivalent `VALIDATOR_COPILOT__AGENTIC__*` environment variables) and supplying an API key:

```toml
[agentic]
provider = "openai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"
# system_prompt = "optional custom instructions"
# temperature = 0.2
```

Export the matching key before starting the agent, e.g. `export OPENAI_API_KEY=sk-...`. When the block is present, the agent will send validator metrics + the detected issue to the model and translate the JSON response into concrete actions. If the provider is not configured or the call fails, the existing rule-based playbooks remain as a safe fallback.

## Prerequisites

- Docker Engine + Compose v2.20+ (for the full local stack)
- Rust 1.84+ if you plan to run binaries outside Docker
- Node 20+ (only needed if you want to run the dashboard locally; Docker handles it otherwise)

## Quick start

Bring up Redis, the Rust services, validator, and the dashboard via Docker:

```bash
cp config.docker.toml config.toml        # optional customization
docker compose up --build \
  agent metrics_collector executor \
  validator_client1 validator_client2 \
  validator1 validator2 dashboard
```

You should now have:

- Agent API on http://localhost:3000 (health, metrics, JSON API)
- React dashboard on http://localhost:5173
- Validator mocks on http://localhost:9101/metrics and http://localhost:9102/metrics

The dashboard refreshes every few seconds by calling:

- `GET /api/validators` list of configured validators, latest metrics (if available), risk score, and derived status (`ok`, rule name, `no_data`, or `invalid_metrics`).
- `GET /api/actions` pending queue length

### gRPC executor control plane

- `executor_daemon` runs next to the control-plane services and hosts a gRPC server (default `0.0.0.0:50051`). It authenticates validator clients, streams actions to them, accepts their results, ingests their metrics, and fans those metrics out to the agent + metrics collector.
- `validator_client` runs on every validator host. It authenticates with its shared secret, receives actions, executes them locally, scrapes local Prometheus-style metrics, and continuously publishes those metrics back to the daemon.
- `agent` and `metrics_collector` never scrape validators or touch Redis directly. They each open a gRPC connection to the daemon: the agent subscribes to live metrics and pushes new remediation actions, while the metrics collector subscribes to the same stream and mirrors it into Redis for the dashboard.
- Environment variables:
  - `EXECUTOR_LISTEN_ADDR` (server) overrides the listen address (`0.0.0.0:50051` default).
  - `EXECUTOR_SERVER_ADDR`, `VALIDATOR_ID`, `VALIDATOR_AUTH_TOKEN`, `VALIDATOR_METRICS_URL` (validator client) control how a validator connects and where it scrapes metrics.
  - `EXECUTOR_SERVER_ADDR` (agent + metrics_collector) points them at the daemon.

### Dashboard preview

![Validator dashboard screenshot](image.png)

## Working locally without Docker

1. Install toolchains:
   ```bash
   rustup default 1.84
   cargo install just   # optional helper
   ```
2. Start Redis (e.g. `brew services start redis`).
3. Copy `config.example.toml` → `config.toml` and update `redis_url` + validator hosts to match your environment.
4. Run each binary:
   ```bash
   EXECUTOR_SERVER_ADDR=http://localhost:50051 cargo run -p metrics_collector
   EXECUTOR_SERVER_ADDR=http://localhost:50051 cargo run -p agent
   cargo run -p executor --bin executor_daemon   # control-plane gRPC server
   # on every validator host
   EXECUTOR_SERVER_ADDR=http://<control-plane>:50051 \
   VALIDATOR_ID=validator-local-1 \
   VALIDATOR_AUTH_TOKEN=validator-local-1-secret \
   VALIDATOR_METRICS_URL=http://127.0.0.1:9100/metrics \
     cargo run -p executor --bin validator_client
   ```
5. For the dashboard:
   ```bash
   cd dashboard
   npm install
   VITE_AGENT_API=http://localhost:3000 npm run dev
   ```

## Running tests

The repo includes unit tests for each crate. Run everything (inside Docker to guarantee toolchain parity) with:

```bash
docker compose run --rm tests \
  sh -c "rustup component add rustfmt && cargo fmt && cargo test"
```

Or run locally if you already have Rust 1.84 installed:

```bash
cargo fmt
cargo test
```

## Repository layout

```
├── Cargo.toml                # Workspace definition + shared deps
├── config.example.toml       # Sample validator + Redis config
├── config.docker.toml        # Config wired to the Compose network
├── crates/
│   ├── agent/
│   ├── common/
│   ├── executor/
│   └── metrics_collector/
├── dashboard/                # React dashboard (Vite)
├── docker/                   # Entrypoint + validator mock server
├── docker-compose.yml
└── docs/
    └── docker.md             # Extended Docker + API notes
```

## Useful endpoints

| Path | Description |
| --- | --- |
| `GET /health` | Simple “ok” response for readiness probes. |
| `GET /api/actions` | pending count, future place for richer action stats. |
| `GET /api/validators` | Validator list including metrics, issue status, and risk score. |
| `GET /dashboard` | Dashboard for looking at current status of validator |