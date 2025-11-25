# Repository Guidelines

This guide keeps contributions consistent while the project structure grows. Treat it as the source of truth for layout, commands, and review expectations.

## Project Structure & Module Organization
- Place production code under `src/` organized by feature or domain (`src/validator/`, `src/http/`, etc.); keep entrypoints in `cmd/` or `apps/` when applicable.
- Mirror code with tests in `tests/` using matching names, and place reusable test data in `tests/fixtures/` or `tests/__mocks__/`.
- Keep automation in `scripts/` (shell or Python) and non-secret configuration in `config/`; add human-readable notes or ADRs in `docs/`.
- Prefer small, well-named modules with clear boundaries; isolate side effects at the edges (I/O, network) to keep core logic pure and testable.

## Build, Test, and Development Commands
- Standardize on Make (or equivalent package scripts) to wrap stack-specific tooling. Add these targets if missing to avoid command drift:
  - `make install` — install dependencies.
  - `make lint` — run static analysis/linters.
  - `make fmt` — auto-format code.
  - `make test` — run the full test suite.
  - `make run` — start the local app/service; add flags for hot reload when available.
- Keep commands fast; prefer watch modes (`npm test -- --watch`, `go test ./...` with `-run` filters) for tight feedback loops.

## Coding Style & Naming Conventions
- Always auto-format before committing (`fmt` target). Use the language-native formatter (`gofmt`, `rustfmt`, `black`, `prettier`, etc.) rather than manual tweaks.
- Default indentation: 2 spaces for web configs/JS/TS; 4 spaces for Python; let Go format with tabs via `gofmt`.
- Naming: `PascalCase` for exported types/classes, `camelCase` for variables/functions, `kebab-case` for scripts, and `snake_case` for Python modules. Keep filenames descriptive (`validator_rules.ts`, `schema_loader.py`).
- Avoid magic values; prefer constants near usage or in a `config` module.

## Testing Guidelines
- Tests should mirror feature boundaries and fail fast. Use table-driven cases for validators and edge conditions.
- Name files `<name>.test.<ext>` (or `_test.go` for Go). Co-locate helpers under `tests/fixtures/`.
- Run `make test` before pushing; target ≥80% coverage where tooling supports it. Add a regression test for every bug fix.

## Commit & Pull Request Guidelines
- Write imperative, concise commit titles (`Add validation for blank input`); include a short body explaining the why when non-trivial. Reference issues with `Refs #123` or `Fixes #123`.
- Keep PRs focused and link the intent, approach, and verification steps. Include screenshots or sample requests/responses when behavior changes.
- Ensure `make lint` and `make test` are clean before review; mark anything intentionally skipped and why.

## Security & Configuration Tips
- Do not commit secrets or personal data. Use `.env.example` to document required variables and load them via tooling like `direnv` or dotenv libraries.
- Document external integrations (API endpoints, keys, test accounts) in `docs/` with redacted values so reviewers can reproduce safely.
