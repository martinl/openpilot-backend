# openpilot-backend

[![CI](https://github.com/martinl/openpilot-backend/actions/workflows/ci.yaml/badge.svg)](https://github.com/martinl/openpilot-backend/actions/workflows/ci.yaml)
[![Coverage](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/martinl/openpilot-backend/badges/coverage.json)](https://github.com/martinl/openpilot-backend/actions/workflows/ci.yaml)

Self-hosted, open-source backend for **openpilot Connect** and **Athena** (Rust 1.100 / Axum 0.8 + SeaORM 1.1 + PostgreSQL 18 + Connect SPA frontend).

Forked from `konik-ai/backend` and adapted for containerized / Kubernetes deployments behind an external TLS gateway.

---

## Container Images (GHCR)

Built and published automatically by [.github/workflows/ci.yaml](.github/workflows/ci.yaml) after all lint, format, security audit, clippy, unit + PostgreSQL integration tests, and the 50%+ line coverage gate (`cargo-llvm-cov`) pass:
- `ghcr.io/martinl/openpilot-backend/backend` ([Dockerfile](Dockerfile) — Rust `connect-cli`, `LOCO_ENV=useradmin`, HTTP port `3112`, internal Prometheus metrics port `9090`)
- `ghcr.io/martinl/openpilot-backend/frontend` ([Dockerfile.frontend](Dockerfile.frontend) — `martinl/openpilot-frontend` SPA on `nginx:stable-alpine`, port `80`)
- `ghcr.io/martinl/openpilot-backend/minikeyvalue` ([Dockerfile.minikeyvalue](Dockerfile.minikeyvalue) — multi-stage `golang:1-alpine` → `nginx:stable-alpine`, non-root `mkv` UID `1000`, single nginx instance, persistent data at `/data`, ports `3000-3005`)

See [CHANGES.md](CHANGES.md) for the full changelog.

Tagging:
- Push to `master`/`main` → `:test` and `:sha-<short>`
- Release tag `v*` → `:vX.Y.Z`, `:X.Y`, and `:latest`

---

## Connecting an openpilot Device

1. SSH into the device.
2. *(Cloned devices only)* Ensure the device has a unique OpenSSL keypair.
3. Clear the existing dongle ID:
   ```bash
   rm -f /data/params/d/DongleId /persist/comma/dongle_id
   ```
4. Export your server URLs in `/data/continue.sh` right before `exec ./launch_openpilot.sh` (persists across reboots and openpilot/sunnypilot OTA updates without modifying `/data/openpilot`):
   ```bash
   export API_HOST=https://api.<your-domain>
   export ATHENA_HOST=wss://athena.<your-domain>
   ```
5. Reboot the device and scan the pairing QR code on `https://connect.<your-domain>`.

---

## Local Development & Container Builds

### 1. Submodules & Pre-commit
```bash
git submodule sync openpilot
git submodule update --init --depth 1 openpilot
git -C openpilot submodule sync opendbc_repo
git -C openpilot submodule update --init --depth 1 opendbc_repo
git submodule update --init --depth 1 frontend minikeyvalue

# Optional local hooks (gitleaks, hadolint, rustfmt, cargo-audit, yaml/CRLF checks)
brew install pre-commit hadolint cargo-audit
pre-commit install
```

### 2. Environment Configuration
Copy [.env_template](.env_template) to `.env` and fill in your endpoints, database credentials, and GitHub OAuth credentials.

### 3. Local Docker Builds
```bash
docker build -t openpilot-backend:local -f Dockerfile .
docker build -t openpilot-frontend:local -f Dockerfile.frontend .
docker build -t openpilot-minikeyvalue:local -f Dockerfile.minikeyvalue .
```

### 4. Native Cargo Tests (macOS)
```bash
rustup default 1.100.0
brew install capnp ffmpeg

# Unit tests only (no database required)
cargo test --lib

# Full unit + integration suite (DB tests auto-skip when TEST_DATABASE_URL is unset)
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres_test \
  cargo test --all-features --workspace
```

---

## Observability (Prometheus & Grafana)

The backend exposes a Prometheus `/metrics` endpoint on an internal-only port (`METRICS_PORT`, default `9090`; set `METRICS_PORT=0` to disable — keep port `9090` cluster-internal and unexposed on public gateways):

- **HTTP RED:** `http_requests_total{method, path, status}`, `http_request_duration_seconds_bucket{method, path}`, `http_requests_in_flight`
- **Fleet & Uploads:** `athena_connected_devices`, `active_uploads`, `active_upload_bytes`
- **Background Workers:** `worker_jobs_total{worker, status}`, `worker_job_duration_seconds_bucket{worker}`

Pre-built Grafana dashboards (for `kube-prometheus-stack` sidecar provisioning or manual import):
- [deploy/grafana/dashboards/openpilot-red.json](deploy/grafana/dashboards/openpilot-red.json) — HTTP Rate, Errors, and Duration (p50/p95/p99, top & slowest routes)
- [deploy/grafana/dashboards/openpilot-app.json](deploy/grafana/dashboards/openpilot-app.json) — Athena fleet, active uploads, worker jobs, PVC usage, and CloudNativePG PostgreSQL panels (enable `spec.monitoring.enablePodMonitor: true` on your CNPG `Cluster`)

### Useradmin Server Usage & Units
- Set `PROMETHEUS_URL` (e.g., `http://kube-prometheus-stack-prometheus.monitoring:9090`) to show live Kubernetes PVC disk usage on the `useradmin` server usage page; `PROMETHEUS_NAMESPACE` is auto-detected from the pod's service account namespace (falls back to local `sysinfo` disks when unset).
- Set `STATS_UNIT=imperial` for miles on `useradmin` and server usage pages (defaults to `metric` / km).
