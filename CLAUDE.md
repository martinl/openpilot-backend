# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Open-source replacement for comma connect — the cloud backend that openpilot-equipped devices pair with, upload telemetry to, and receive live JSON-RPC commands from. Public instance is https://stable.konik.ai. For a full architectural tour (routes, data model, workers, storage, auth) see [ARCHITECTURE.md](ARCHITECTURE.md); **read it first** before making non-trivial changes.

## Prerequisites & setup

- Submodules are required (`openpilot/` provides cereal schemas, `capnproto-rust/` is a `[patch.crates-io]` library fork, `frontend/` is the SolidJS PWA, `minikeyvalue/` is the Go blob store). Run `git submodule update --init` after clone — many things fail silently without this (`src/build.rs` needs `openpilot/cereal/*.capnp`).
- System deps for `ffmpeg-next`: `pkg-config libavutil-dev libavformat-dev libavcodec-dev libavdevice-dev libclang-dev clang`. [setup.sh](setup.sh) installs these plus Rust/Node/Docker on Debian/Ubuntu.
- Copy `.env_template` to `.env` and fill in `SERVER_SECRET`, `GITHUB_CLIENT`/`GITHUB_SECRET`, `ADMIN_JWT`, etc. `DATABASE_URL`, `REDIS_URL`, and `MKV_ENDPOINT` are read by both the Rust binary and compose.
- TLS certs must exist at `self_signed_certs/cert.pem` + `key.pem` — [src/app.rs:182-191](src/app.rs#L182-L191) unconditionally loads them, so the binary panics at boot if they're missing. Generate self-signed if needed.

## Common commands

### Build & run (local)

```bash
cargo build                      # debug
cargo build --release            # prod profile (LTO fat, codegen-units=1 — slow)
cargo run --bin connect-cli -- start           # boot the server (dual HTTP 3111 / HTTPS 3222)
cargo run --bin connect-cli -- start -e production
```

The binary is always `connect-cli` (see `[[bin]]` in [Cargo.toml](Cargo.toml)). Loco CLI subcommands: `start`, `db {migrate,reset,entities,status}`, `task <name>`, `routes`, `doctor`.

### Database (Sea-ORM via Loco + migration crate)

```bash
# apply migrations from the embedded Migrator (also auto-runs at boot per config/connect.yaml)
cargo run --bin connect-cli -- db migrate

# regenerate _entities/ from the live schema after adding a migration
cargo run --bin connect-cli -- db entities

# nuke + rebuild schema, then seed (combined in fresh_seeds.sh)
cd migration && cargo run -- fresh && cd ..
cargo loco task seed_data
```

Migrations live in [migration/src/](migration/src/) and are registered in `migration/src/lib.rs`. Writing a new migration: add a file `mXXXXXXXX_*.rs`, add it to `Migrator::migrations()`, then re-run `db migrate` followed by `db entities`.

### Background tasks (CLI-invoked, registered in [src/app.rs:90-98](src/app.rs#L90-L98))

```bash
cargo loco task <name>
# or in production:
./target/release/connect-cli task <name>
```

Names: `storage_count`, `deleter`, `cleaner`, `collect_data`, `seed_from_mkv`, `seed_data`, `huggingface`. `deleter` is wired to hourly cron inside [Dockerfile](Dockerfile).

### Workers

Sidekiq workers (`bootlog_parser`, `log_parser`, `jpg_extractor`) run in-process when `workers.mode` is `ForegroundBlocking` ([config/connect.yaml:94](config/connect.yaml#L94)), so `start` is enough in dev. Redis is still required.

### Tests

Barely any exist — only [src/middleware/jwt.rs](src/middleware/jwt.rs) has unit tests. Run with `cargo test`. Dev-deps include `rstest`, `serial_test`, `insta` (snapshot testing) — wire those up when adding tests. There is **no** `tests/` integration dir.

### Docker

```bash
docker compose up -d --build     # full stack: postgres, redis, minikeyvalue, connect, nginx
./deploy.sh                      # one-liner wrapper for the above
```

The compose file uses `network_mode: "host"` on all services and bind-mounts `/mnt/raid5/{kvstore1,params}` — adjust for your host before first run.

### Hot reload against a running container

`hot_load_frontend.sh` copies `frontend/dist/` into the `connect-killer-connect-1` container and kill-restarts the binary. `hot_load_assets.sh` does the same for `assets/` + `config/`. Both assume the container name and the `start_connect.sh`/`start_useradmin.sh` entry scripts — useful when iterating against a prod-like container without full rebuild.

### Frontend

`frontend/` is a separate submodule (SolidJS + Vite, uses `bun`). Loco serves its built output from `frontend/dist/` via the `static` middleware ([config/connect.yaml:79-86](config/connect.yaml#L79-L86)). To work on it: `cd frontend && bun install && bun dev` (see `frontend/README.md`). The Rust server does not need to be rebuilt when the frontend changes — just rebuild `frontend/dist/`.

## Architecture cheat-sheet

The full picture is in [ARCHITECTURE.md](ARCHITECTURE.md). Key invariants a change-maker needs to know:

- **One binary, many surfaces.** `connect-cli` is a single Loco app whose routes are mounted per prefix in [src/app.rs:65-79](src/app.rs#L65-L79): `/v1` (client API), `/v2` (device auth), `/ws` (Athena WebSocket), `/connectincoming` (PUT uploads), `/connectdata` (GET downloads), `/useradmin`, `/maps`, `/params`, `/stats`. Add new endpoints inside an existing controller rather than a new top-level prefix unless the semantics differ.
- **Auth is bimodal.** A JWT's `identity` claim is either a `dongle_id` (device) or a user UUID. The `MyJWT` extractor in [src/middleware/auth.rs](src/middleware/auth.rs) resolves this and attaches the right model. Don't parse JWTs manually.
- **Device registration pins `dongle_id`.** `dongle_id = sha256(imei|imei2|serial|pubkey)[..16]` — changing this formula invalidates every existing pairing. The register_token is verified against the device's *submitted* public key (RS256 or ES256) in [controllers/v2.rs](src/controllers/v2.rs).
- **Blob storage is minikeyvalue, not the DB.** qlogs, segments, videos, bootlogs live in mkv under keys like `{dongle_id}_segment_{canonical_name}`. `PUT /key` blocks until replicated; `GET /key` returns a 302 to an nginx volume server. Use helpers in [src/common/mkv_helpers.rs](src/common/mkv_helpers.rs) to build URLs — don't concat `MKV_ENDPOINT` yourself.
- **Upload URLs are HMAC-signed.** `POST /v1/{dongleId}/upload_urls` (in [controllers/v1.rs](src/controllers/v1.rs)) returns pre-signed mkv PUT URLs; the device uploads directly. If `UPLOAD_ENDPOINT` is set in `.env`, it overrides `API_ENDPOINT` for just this purpose (used to route around proxy timeout limits — see README.md).
- **Capnproto schemas are compiled at build time.** [src/build.rs](src/build.rs) compiles `openpilot/cereal/{log,car,custom,legacy}.capnp` into `src/cereal/` and also scrapes the `Event` union to emit `src/cereal/generated_event_type_names.rs`. If you touch `log.capnp` or bump the `openpilot/` submodule, that match table regenerates — check downstream `LogEvent::Which::*` usages. See the "Schema & Fork Compatibility" section of ARCHITECTURE.md for how fork schemas are (and aren't) handled.
- **Entities are generated.** `src/models/_entities/` is output from `db entities` — do not hand-edit. Domain logic belongs in the sibling `src/models/<name>.rs` files (e.g. [src/models/routes.rs](src/models/routes.rs)).
- **In-process tokio loops.** `App::after_routes` ([src/app.rs:121-172](src/app.rs#L121-L172)) spawns two permanent loops: a 10 s WebSocket ping to all devices and a 60 s param-snapshot persist. Both are load-bearing for the `online` flag and param admin UI — don't remove casually.
- **`loco-rs` environments are enum-matched.** The `Environment::Any("connect".to_string())` branch in [src/app.rs:66-67](src/app.rs#L66-L67), [82-83](src/app.rs#L82-L83), and [125-126](src/app.rs#L125-L126) is a stripped-down variant that skips routes/workers/WS — used for CLI-only invocations. Know which branch your change lives in.

## Non-obvious gotchas

- `connect-killer` is the old project name; compose container name `connect-killer-connect-1`, docker volumes `connect-killer_pgdata`/`connect-killer_redisdata` still carry it. Don't rename blindly.
- `network_mode: "host"` in compose means port mappings in `ports:` are cosmetic — actual binding is controlled by the binary listening on 0.0.0.0.
- Default `workers.mode: ForegroundBlocking` ([config/connect.yaml:94](config/connect.yaml#L94)) runs Sidekiq in-process; switching to `BackgroundQueue` requires a separate worker process.
- The README's "Hosting your own instance (Hardcore)" disclaimer is accurate — there are hardcoded values (domain, server IPs, STUN/TURN) in controllers that need patching for a new deployment.
