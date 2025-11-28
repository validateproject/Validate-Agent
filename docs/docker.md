# Docker-based local testing

This repository now ships with a docker-compose workflow that spins up every runtime dependency plus a containerized Rust toolchain. It lets you:

- bring up Redis, the metrics collector, the agent, and the executor in one command, and
- run `cargo test` without installing Rust locally (the `tests` service handles that for you).

## Prerequisites

- Docker Engine and Docker Compose Plugin v2.20+ (the file uses health checks and profiles available in the v2 syntax)
- Rust 1.84+ if you plan to run `cargo` locally (the containers already ship with 1.84)
- A copy of `config.docker.toml` on your host (edit validator entries as needed)

## Files

- `Dockerfile` – builds all workspace binaries (`agent`, `metrics_collector`, `executor_daemon`)
- `config.docker.toml` – sample config wired to the Compose network (`redis://redis:6379`)
- `docker-compose.yml` – defines runtime services, mock validator hosts, the React dashboard, plus a throwaway `tests` runner
- `dashboard/` – Vite + React frontend that consumes the agent API (`/api/validators`, `/api/actions`)

## Quick start

1. Copy/edit the Docker config:
   ```bash
   cp config.docker.toml config.toml
   # or keep config.docker.toml as-is and let Compose mount it read-only
   ```
2. Build everything and launch the runtime stack (validators + dashboard included):
   ```bash
   docker compose up --build agent metrics_collector executor validator1 validator2 dashboard
   ```
   Redis is started automatically (and exposed on `localhost:6379`), and the agent's HTTP server is published on `localhost:3000`.
   The validator mocks expose Prometheus-style metrics on `localhost:9101/metrics` and `localhost:9102/metrics` and are reachable inside the Compose network via `http://validator1.local:9100/metrics` and `http://validator2.local:9100/metrics`.
3. Tail logs or inspect pending actions:
   ```bash
   curl localhost:3000/health
   curl localhost:3000/debug/actions/pending
   ```
4. Open http://localhost:5173 to view the React dashboard (served by the `dashboard` service). It calls `http://localhost:3000/api/validators` and `/api/actions` every few seconds to stay in sync.

### Agent API endpoints

- `GET /api/validators` – list of configured validators, their most recent metrics (if any), computed risk score, and derived status (`ok`, issue name, `no_data`, or `invalid_metrics`).
- `GET /api/actions` – JSON payload that currently exposes the `pending` queue length; the legacy `/debug/actions/pending` path is still available for scripts.

## Running the Rust test suite inside Docker

Use the dedicated `tests` service so you do not need a local Cargo installation:

```bash
docker compose run --rm tests
```

The service mounts the repository into `/workspace`, caches dependencies via two named volumes (`cargo-cache` and `target-cache`), and executes `cargo test --workspace`.

## Cleaning up

```bash
docker compose down
docker volume rm validate_cargo-cache validate_target-cache  # optional cache purge
```

> Tip: pass `-d` to `docker compose up` to daemonize, or `--profile tests` if you want to extend the file with more ad-hoc tooling later.

## Accessing validator hosts from the host machine

Inside Docker, services reach the mock validators via their `.local` hostnames. If you want to send requests from your machine without going through the mapped ports, add the following to `/etc/hosts`:

```
127.0.0.1 validator1.local
127.0.0.1 validator2.local
```

Otherwise, use the published ports (`http://localhost:9101/metrics`, `http://localhost:9102/metrics`) which always work without modifying hosts.

