# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), covering active development on this fork under `[Unreleased]` followed by the upstream history from the initial commit.

## [Unreleased]

### Added
- Multi-stage container images (`Dockerfile`, `Dockerfile.frontend`) with non-root runtime configuration and startup runtime config injection (`config.js`) for immutable promotion across environments.
- GitHub Actions CI/CD workflow (`.github/workflows/ci.yaml`) with `rustfmt`, `clippy`, `cargo-audit`, PostgreSQL 18 integration tests, `cargo-llvm-cov` coverage reporting (`--fail-under-lines 50` gate and Shields.io badge), Trivy image scans, and GHCR image publishing on `main`/`master` merges and release tags.
- Frontend Docker build metadata wiring (`VITE_APP_GIT_SHA`, `VITE_APP_GIT_TIMESTAMP`) from the `frontend` submodule into `BuildInfo`.
- Expanded test suite from 13 to 54 unit tests plus end-to-end PostgreSQL 18 integration tests (`tests/models_integration.rs`, `tests/v1_and_admin_api.rs`, `tests/connect_io_api.rs`) backed by an in-process `MockMkvServer`.
- Comma.ai OpenAPI 3.1 contract test suite (`tests/openapi_contract.rs` against vendored `tests/fixtures/comma-openapi.json`) validating implemented routes and live JSON responses.
- Pre-commit configuration (`.pre-commit-config.yaml`) including RustSec `cargo-audit` checks.
- Parameterized environment configuration for OAuth, cookies (`COOKIE_NAME`, `COOKIE_DOMAIN`), CORS origins, optional internal TLS, and Connect frontend endpoints.

### Changed
- Replaced `loco-rs` framework dependency with an in-tree Axum-based runtime (`src/framework.rs`) and dropped `ffmpeg-next`, `libpq`, `libssl`, unmaintained `dotenv` / `async-std`, and unused crates (`bzip2`, `combine`, `hyperlocal`, duplicate `capnpc`).
- Upgraded Rust toolchain to `1.100` and modernized core dependencies: `axum 0.8`, `axum-extra 0.10`, `axum-server 0.9`, `tower-http 0.6`, `sea-orm 1.1`, `reqwest 0.13` (`rustls`), `jsonwebtoken 12` (`aws_lc_rs`), `tokio-tungstenite 0.30`, `rand 0.10`, `sha2 0.11`, `hmac 0.13`, `validator 0.21`, `base64 0.23`, `dotenvy`, and `tokio`.
- Renamed `Stable` branding to `Connect` and pointed the `frontend` submodule to `martinl/openpilot-frontend`.
- Resolved compiler and Clippy warnings across `src/`, `src/build.rs`, and `tests/`.

### Fixed
- Axum 0.8 root routing compatibility by switching from root `nest_service` to `fallback_service`.
- `Routes::join_path` dot-prefixed route concatenation so `/v1.1/...` and `/v1.4/...` paths resolve without inserting an extra slash.
- `ROUTE_NAME` regex alternation grouping so bootlog downloads capture `dongle_id` on wall-clock timestamps.
- Six OpenAPI 3.1 response schema drifts across `v1` endpoints (`MeResponse.user_id`, `last_gps_accuracy`, `device_type` in device info/alias updates, `saved_next: true` on destination updates, and `deleted` payload on `DELETE /v1/navigation/{dongle_id}/next`).
- `athenad` WebSocket authentication fallback to the default `jwt` cookie when a custom `COOKIE_NAME` is configured, and registered `GET /v1/devices/{dongle_id}`.
- Mounted Athena JSON-RPC on root `POST /{dongle_id}` alongside `POST /ws/{dongle_id}` to match OpenAPI and Connect frontend clients.
- Unauthenticated and invalid-JWT requests on `useradmin-*` subdomains now redirect to `/login` instead of returning `401`.
- Set `Content-Type: application/vnd.apple.mpegurl` on HLS `qcamera.m3u8` playlists and bumped `frontend` to prefer native HLS on Safari (fixing black video playback on macOS Safari) and fix QR scanner pairing.

### Security
- Upgraded `url` to `2.5.9` (`RUSTSEC-2026-0592`) and `cookie` to `0.18.3` (`RUSTSEC-2026-0594`).
- Removed hardcoded upstream domains, IP addresses, and default JWT signing secrets.

---

## Upstream History (`konik-ai/backend`, 2024-04 – 2026-03)

### 2026-01 – 2026-03
- Added device sharing endpoints (`GET /v1/devices/{dongle_id}/users`, `add_user`, `del_user`) and updated `useradmin` and login templates.
- Refactored Athena WebSocket device connections to use `mpsc::Sender` with improved error handling.
- Added Bun frontend build support, `rustls` crypto provider initialization, and switched the `openpilot` submodule to upstream `openpilot`.

### 2025-07 – 2025-11
- Added server usage statistics page with time-series charts, `cleaner` retention task, `firehose` device toggle, upload queue tracking, and storage accounting.
- Added nginx `docker-compose` configuration, dynamic API host URLs in `useradmin` templates, and HLS segment start offset calculation for modal video playback.

### 2025-01 – 2025-05
- Integrated `new-connect` SPA frontend support, Mapbox proxy with per-user rate limiting, cloudlog cache (`swaglog`), and `/params` analytics.
- Added `rlog.zst` on-demand upload support, route `git_remote` / `git_commit` / `git_branch` metadata, and Prometheus metrics.

### 2024-08 – 2024-12
- Added Zstandard (`.zst`) log compression support and resilient route timestamp calculation when GPS time is unavailable.
- Added Dockerization, drive stats, last known device location, and navigation endpoints (`set_destination`, saved locations).
- Hardened endpoint and file download authentication, added retention deletion tasks, and upgraded `loco-rs` to `0.8`.

### 2024-04 – 2024-06 (Initial Foundation)
- Initial Rust backend with SeaORM PostgreSQL models (`users`, `devices`, `authorized_users`, `routes`, `segments`, `bootlogs`) and Cap'n Proto `cereal` log parser.
- Implemented `connectincoming` / `connectdata` storage pipeline with `minikeyvalue`, Athena WebSockets, GitHub OAuth, `pilotauth` device registration, `useradmin`, HLS video playback, monotonic route naming, and compatibility with `openpilot` tools and Cabana.
