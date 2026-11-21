# openpilot-backend

[![CI](https://github.com/martinl/openpilot-backend/actions/workflows/ci.yaml/badge.svg)](https://github.com/martinl/openpilot-backend/actions/workflows/ci.yaml)
[![Coverage](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/martinl/openpilot-backend/badges/coverage.json)](https://github.com/martinl/openpilot-backend/actions/workflows/ci.yaml)

Self-hosted, open-source backend for **openpilot Connect** and **Athena** (Rust / Axum 0.8 + SeaORM 1.1 + PostgreSQL 18 + Connect SPA frontend).

Forked from `konik-ai/backend` and adapted for containerized / Kubernetes deployments behind an external TLS gateway.

---

## Container Images (GHCR)

Built and published automatically by [.github/workflows/ci.yaml](.github/workflows/ci.yaml) after all lint, format, security audit, clippy, and unit/doc tests pass:
- `ghcr.io/martinl/openpilot-backend/backend` ([Dockerfile](Dockerfile) — Rust `connect-cli`, `LOCO_ENV=useradmin`, port `3112`)
- `ghcr.io/martinl/openpilot-backend/frontend` ([Dockerfile.frontend](Dockerfile.frontend) — `martinl/openpilot-frontend` SPA on `nginx:stable-alpine`, port `80`)
- `ghcr.io/martinl/openpilot-backend/minikeyvalue` (`minikeyvalue/Dockerfile` — blob storage, ports `3000-3005`)

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
4. Export your server URLs in `launch_openpilot.sh`:
   ```bash
   #!/usr/bin/bash
   export API_HOST=https://api.<your-domain>
   export ATHENA_HOST=wss://athena.<your-domain>
   exec ./launch_chffrplus.sh
   ```
5. Commit your change (`git commit -a -m "switch to custom connect backend"`), disable automatic updates if applicable, reboot, and scan the pairing QR code on `https://connect.<your-domain>`.

---

## Local Development & Container Builds

### 1. Submodules & Pre-commit
```bash
git submodule sync openpilot
git submodule update --init --depth 1 openpilot
git -C openpilot submodule sync opendbc_repo
git -C openpilot submodule update --init --depth 1 opendbc_repo
git submodule update --init --depth 1 frontend minikeyvalue

# Optional local hooks (gitleaks, hadolint, rustfmt, yaml/CRLF checks)
brew install pre-commit hadolint
pre-commit install
```

### 2. Environment Configuration
Copy [.env_template](.env_template) to `.env` and fill in your endpoints, database credentials, and GitHub OAuth credentials.

### 3. Local Docker Builds
```bash
docker build -t openpilot-backend:local -f Dockerfile .
docker build -t openpilot-frontend:local -f Dockerfile.frontend .
docker build -t openpilot-minikeyvalue:local -f minikeyvalue/Dockerfile ./minikeyvalue
```

### 4. Native Cargo Tests (macOS)
```bash
rustup default 1.90.0
brew install pkg-config libpq capnp ffmpeg
RUST_MIN_STACK=134217728 cargo test --lib
```
