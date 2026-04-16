# Architecture Overview

Self-hosted backend for [openpilot](https://github.com/commaai/openpilot)-equipped
devices — an open-source alternative to comma connect. Devices pair with the
server, stream telemetry (qlogs, videos, segments) into a blob store, accept
remote commands over a WebSocket JSON-RPC channel (Athena), and surface drives
through a web dashboard.

## Stack

- **Language**: Rust (edition 2021), single crate `connect` with a local
  `migration` sub-crate. See [Cargo.toml](Cargo.toml).
- **Framework**: [Loco-rs](https://loco.rs) `0.8.1` on top of Axum `0.7`
  (features: `multipart`, `ws`). Async runtime: Tokio `1.40`.
- **Database**: PostgreSQL `15.3` via Sea-ORM `1.0.0-rc.1`
  (sqlx-tokio-rustls backend). Migrations in [migration/src/](migration/src/).
- **Queue**: Redis `7` as the broker for `rusty-sidekiq` `0.8.2` workers.
- **Blob store**: [minikeyvalue](minikeyvalue/) — a Go-based distributed KV
  blob store (LevelDB index, nginx volume servers, optimized for 1 MB–1 GB
  values). Master on port 3000, volume nodes on 3001–3005.
- **TLS**: `axum-server` + Rustls (self-signed certs in `self_signed_certs/`);
  nginx terminates TLS in production.
- **Data plane**: `capnp` `0.19` for the openpilot cereal schema (custom fork
  via `[patch.crates-io]`), `ffmpeg-next` `7.0.4` for video, `bzip2`/`zstd` via
  `async-compression` for logs.
- **Auth**: `jsonwebtoken` `9` (HS512) + RSA/ECDSA public-key verification for
  device pairing.
- **Binary**: `connect-cli` at [src/bin/main.rs](src/bin/main.rs).

## Repository Layout

| Path | Purpose |
|------|---------|
| [src/app.rs](src/app.rs) | Loco `Hooks` impl: boots app, mounts routes, registers workers/tasks, installs middleware, runs dual HTTP/HTTPS listeners. |
| [src/build.rs](src/build.rs) | Generates Rust code from openpilot's `*.capnp` schemas at build time. |
| [src/cereal/](src/cereal/) | Generated capnproto bindings (log, car, custom, legacy). |
| [src/controllers/](src/controllers/) | HTTP/WebSocket route handlers (see routes table below). |
| [src/models/](src/models/) | Domain logic; `_entities/` holds Sea-ORM generated stubs. |
| [src/middleware/](src/middleware/) | JWT extractor, device/user auth resolution. |
| [src/workers/](src/workers/) | Background Sidekiq workers (log parsing, frame extraction). |
| [src/tasks/](src/tasks/) | CLI-invoked jobs (cleanup, seeding, data collection). |
| [src/common/](src/common/) | Helpers: upload tracker, mkv URL builder, regex, enforce guards. |
| [src/views/](src/views/) | JSON response formatters. |
| [src/initializers/](src/initializers/) | Loco initializer hooks. |
| [migration/](migration/) | Sea-ORM migration crate. |
| [config/](config/) | Loco YAML configs per environment (`connect.yaml`, `production.yaml`, `test.yaml`, `useradmin.yaml`). |
| [fetch/](fetch/) | Python data ingestion scripts (HuggingFace, etc.). |
| [frontend/](frontend/) | Git submodule — SolidJS + Vite PWA (Konik Stable, forked from `commaai/new-connect`); leaflet, `hls.js`, Sentry, bun. Deployed at stable.konik.ai. |
| [minikeyvalue/](minikeyvalue/) | Git submodule — Go master + nginx volume servers, LevelDB index. |
| [my-comma-auth/](my-comma-auth/) | Git submodule — small JS helper for comma access-token storage (frontend auxiliary, unused by the Rust server). |
| [openpilot/](openpilot/) | Git submodule — source of `cereal/*.capnp` schemas. |
| [capnproto-rust/](capnproto-rust/) | Git submodule — patched capnp fork (`format-text` branch). |
| [assets/](assets/) | Static assets (maps, etc.). |
| [Dockerfile](Dockerfile), [docker-compose.yml](docker-compose.yml) | Container build + service composition. |
| [nginx.conf](nginx.conf) | Public reverse proxy, TLS, rate limiting, caching. |
| [setup_storage.sh](setup_storage.sh) | WireGuard + NFS setup for distributed `/mnt/raid5`. |

## HTTP & WebSocket Surface

`App::routes` in [src/app.rs:65-79](src/app.rs#L65-L79) mounts one router per
top-level path prefix:

| Prefix | Module | Purpose |
|--------|--------|---------|
| `/ws` | [controllers/ws.rs](src/controllers/ws.rs) | Device WebSocket; Athena JSON-RPC for commands & responses. |
| `/v2` | [controllers/v2.rs](src/controllers/v2.rs) | Auth: `pilotauth` (device registration), `pilotpair`, GitHub OAuth, `user/token`. |
| `/useradmin` | [controllers/useradmin.rs](src/controllers/useradmin.rs) | Admin CRUD for users & device pairing. |
| `/connectincoming` | [controllers/connectincomming/](src/controllers/connectincomming/) | `PUT` ingest for bootlogs, qlogs, segment files (pre-signed URL targets). |
| `/connectdata` | [controllers/connectdata.rs](src/controllers/connectdata.rs) | `GET` download proxy to minikeyvalue. |
| `/v1` | [controllers/v1.rs](src/controllers/v1.rs) | Main client API (see below). |
| `/maps` | [controllers/maps.rs](src/controllers/maps.rs) | Map tile serving. |
| `/params` | [controllers/params.rs](src/controllers/params.rs) | Device parameter snapshots. |
| `/stats` | [controllers/stats.rs](src/controllers/stats.rs) | Aggregate fleet stats. |

`v1` is the workhorse — representative endpoints:
`GET /v1/me`, `GET /v1/me/devices`, `GET /v1/me/jwt`,
`GET|PATCH /v1/route/:fullname` (+ files, QCam stream, share signature),
`POST /v1/{dongleId}/upload_urls` (issues HMAC-signed mkv PUT URLs),
`GET /v1/devices/:dongle_id/routes|routes_segments|stats|users`,
`POST /v1/devices/:dongle_id/{add_user,del_user,unpair}`,
`POST /v1/navigation/:dongle_id/set_destination`,
`GET /v1/iceservers` (WebRTC STUN/TURN).

### Middleware

Applied in `App::after_routes` at [src/app.rs:121-172](src/app.rs#L121-L172):

- `NormalizePathLayer` — trims trailing slashes.
- `Extension<reqwest::Client>` — shared HTTP client with 500-connection pool.
- `Extension<Arc<ConnectionManager>>` — registry of live device WebSockets.
- `Extension<UploadTracker>` — in-memory active-upload byte counters.
- `MyJWT` extractor in [middleware/auth.rs](src/middleware/auth.rs) validates
  JWTs from `Authorization: JWT …`, `?sig=` / `?access_token=` query, or `jwt`
  cookie, then loads the associated `device` or `user` model.

## Data Model

All schema changes live in [migration/src/](migration/src/) and are applied by
the `Migrator` wired into `App::boot`. The tables and their load-bearing
columns:

- **users** — `identity` (UUID PK), `email`, `password`, `name`, `superuser`,
  `locations` (JSON).
- **devices** — `dongle_id` (PK), `owner_id` → users, `public_key`, `online`,
  `server_storage` (bytes), `firehose`, `locations`, timestamps.
- **routes** — `fullname` (PK), `device_dongle_id`, `start_/end_time_utc_millis`,
  `start_/end_lat/lng`, `length`, `share_sig`, segment number/time arrays,
  flags (`proclog`, `proccamera`, `hpgps`, `can`).
- **segments** — `canonical_name` (PK), route back-ref, timestamps, per-camera
  availability flags (`qlog`, `qcam`, `fcam`, `dcam`, `ecam`, `rlog`).
- **authorized_users** — M2M sharing between devices and users.
- **bootlogs** — boot event records.
- **device_msg_queues** — outgoing JSON-RPC requests to devices
  (`request_id`, `method`, `params`).
- **anonlogs** — opt-in anonymous telemetry.

Later migrations add `server_storage`, `locations` JSON, and `firehose` columns
to `devices`.

## Background Work

**Sidekiq workers** (registered in [src/app.rs:85-87](src/app.rs#L85-L87),
Redis-backed):

- `BootlogParserWorker` — parses capnproto bootlogs, enqueues log parsing.
- `LogSegmentWorker` — parses qlogs, extracts segments, computes GPS/camera
  stats, writes to DB.
- `JpgExtractorWorker` — extracts JPEG frames from segment videos.

**CLI tasks** (registered in [src/app.rs:90-98](src/app.rs#L90-L98), invoked
via `connect-cli task <name>`):

- `storage_count` — aggregates mkv bytes per device.
- `deleter` — retention cleanup (wired to hourly cron in [Dockerfile](Dockerfile)).
- `cleaner` — removes stale DB rows.
- `collect_data` — issues `listDataDirectory` JSON-RPC to devices, pulls files.
- `seed_from_mkv` — ingests routes/segments from an existing mkv store.
- `seed` — YAML fixture loader.
- `huggingface` — pushes datasets to HF Hub.

**In-process loops** (spawned in `after_routes`):

- Every 10 s: ping all connected device WebSockets
  ([src/app.rs:137-143](src/app.rs#L137-L143)).
- Every 60 s: `persist_param_value_counts` + `persist_device_params` to
  storage ([src/app.rs:145-156](src/app.rs#L145-L156)).

## Storage

- **Blobs → minikeyvalue.** `PUT /key` blocks until replicated; `GET /key`
  returns a 302 redirect to the nginx volume server holding that replica.
  Devices upload directly via pre-signed URLs issued by
  `/v1/{dongleId}/upload_urls`; server-side ingest through
  `/connectincoming/...` also forwards into mkv. Downloads stream through
  `/connectdata/...` → mkv. The master endpoint is configured by `MKV_ENDPOINT`.
- **Local storage driver.** Loco's `storage::Storage::single(local::new())` is
  installed in [src/app.rs:114-119](src/app.rs#L114-L119) for small server-side
  artefacts (param snapshots, cached images).
- **Distributed disk.** [setup_storage.sh](setup_storage.sh) wires WireGuard +
  NFS so that `/mnt/raid5` is shared across hosts; minikeyvalue and the
  `params` cache bind-mount from there in [docker-compose.yml](docker-compose.yml).

## Auth Flow

1. **Device pairing** ([controllers/v2.rs](src/controllers/v2.rs)) — device
   posts `imei`, `imei2`, `serial`, `public_key` (RSA 2048 or EC), and a
   `register_token` (self-signed JWT). Server verifies the JWT against the
   supplied public key (RS256 or ES256), derives
   `dongle_id = sha256(imei|imei2|serial|pubkey)[..16]`, and returns a device
   JWT signed with the server's HS512 secret.
2. **User auth** — email/password or GitHub OAuth (`GITHUB_CLIENT`,
   `GITHUB_SECRET`); issues a user JWT with the user UUID as `identity`.
3. **Request auth** — `MyJWT` extractor resolves the `identity` claim to either
   a `device` (by `dongle_id`) or `user` (by UUID). `superuser=true` bypasses
   per-device ownership checks; normal users see only devices they own or have
   been added to via `authorized_users`.

## Infrastructure

[docker-compose.yml](docker-compose.yml) defines the runtime services:

- `postgres:15.3-alpine` (5432)
- `redis:7-alpine` (6379)
- `minikeyvalue` (3000–3005)
- `connect` — the Rust binary, HTTP 3111 / HTTPS 3222 (+ useradmin on 3112/3223)
- `nginx:alpine` — public entrypoint

[nginx.conf](nginx.conf) terminates TLS 1.2/1.3 on 443 and proxies
`stable.konik.ai`, `api.konik.ai`, and `useradmin.konik.ai` to the respective
Rust listeners. Uploads are rate-limited (`limit_req_zone … rate=2r/s`) and
static assets are cached to `/dev/shm/nginx-cache` (RAM).

[Dockerfile](Dockerfile) builds on Ubuntu 20.04 with Rust 1.88, Node, Bun,
FFmpeg, and capnproto; it pre-installs `loco-cli`, `cargo-insta`, and
`sea-orm-cli`, and registers an hourly cron entry for the `deleter` task.

## External Integrations

- **openpilot/cereal** — schemas compiled at build time; this is the on-wire
  and on-disk format for all device telemetry.
- **GitHub OAuth** — user login path in `v2`.
- **HuggingFace Hub** — `tasks::huggingface` uploads with `HF_TOKEN`.
- **STUN/TURN servers** — hardcoded list in
  [controllers/v1.rs](src/controllers/v1.rs) for WebRTC ICE negotiation.
- **Mapbox** — referenced from [controllers/maps.rs](src/controllers/maps.rs).

## Schema & Fork Compatibility

Devices in the wild run many openpilot forks (sunnypilot, frogpilot,
dragonpilot, …). Capnproto's evolution rules decide what the backend can and
can't do with their telemetry.

- **What the backend compiles against.** The pinned [openpilot/](openpilot/)
  submodule provides `cereal/{log,car,custom,legacy}.capnp`, compiled by
  [src/build.rs](src/build.rs) into [src/cereal/](src/cereal/).
  `build.rs` additionally scrapes `log.capnp`'s `Event` union and emits
  `src/cereal/generated_event_type_names.rs` with one match arm per variant —
  anything not in the pinned `Event` union is invisible to event-type dispatch.
- **Why forks mostly just work.** Capnproto guarantees 64-bit unique type IDs,
  explicit field ordinals, and unknown-field preservation. A fork that *adds*
  fields with fresh ordinals is wire-compatible; the backend parses those
  messages without error.
- **What is not decoded.** Fork-added fields travel through intact but have no
  Rust accessor here, so they don't land in `segments`, `routes`, or stats.
- **The one foot-gun.** Two forks independently claiming the same ordinal in
  the same struct with different types will silently misdecode each other's
  messages — capnproto does not detect collisions. Coordination means reserving
  ordinal ranges, or putting extensions inside a new struct in `custom.capnp`.
- **To decode fork fields.** Either vendor the fork's schema diff into this
  repo's `custom.capnp`, or compile multiple schema variants and dispatch by
  device fingerprint. Not currently implemented.
- **Note on [capnproto-rust/](capnproto-rust/).** That submodule is a fork of
  the capnp *library* (`format-text` branch, used via `[patch.crates-io]`) —
  orthogonal to schema forks.

## Request Lifecycle (example: device uploads a qlog segment)

1. Device pairs via `/v2/pilotauth`, receives device JWT.
2. Before rotating a segment, device calls
   `POST /v1/{dongle_id}/upload_urls` → server returns HMAC-signed mkv PUT URLs.
3. Device PUTs the bzip2/zstd-compressed qlog directly to minikeyvalue (or via
   `/connectincoming/...`, which relays).
4. An ingestion path enqueues `BootlogParserWorker` → `LogSegmentWorker`
   (Sidekiq via Redis).
5. `LogSegmentWorker` downloads the blob, decodes capnproto frames, upserts
   `routes` + `segments` rows, and updates device stats.
6. The web frontend calls `GET /v1/devices/:dongle_id/routes_segments`,
   streams QCam through `GET /v1/route/:fullname/qcamera.m3u8`, and pulls
   individual files through `/connectdata/...`.
7. For live device commands (reboot, uploadFileToUrl, takeSnapshot) the
   frontend opens a user-authenticated channel; the server forwards the
   JSON-RPC request over the device's `/ws` connection via `ConnectionManager`
   and returns the response.
