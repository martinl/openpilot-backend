# syntax=docker/dockerfile:1.7

# =============================================================================
# Stage 1: Build connect-cli (Rust + Cap'n Proto + FFmpeg dev libs)
# =============================================================================
FROM rust:1.100-bookworm AS builder

ENV DEBIAN_FRONTEND=noninteractive
ENV CC=clang
ENV CXX=clang++
ENV RUST_MIN_STACK=134217728

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    clang \
    libclang-dev \
    pkg-config \
    libpq-dev \
    libssl-dev \
    libavutil-dev \
    libavformat-dev \
    libavcodec-dev \
    libavdevice-dev \
    libavfilter-dev \
    libswscale-dev \
    libswresample-dev \
    capnproto \
    git \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/connect

# Copy manifests and openpilot schemas (including nested submodule targets)
COPY Cargo.toml Cargo.lock* ./
COPY migration/Cargo.toml migration/Cargo.toml
COPY openpilot/ openpilot/
COPY src/build.rs src/build.rs

# Copy full source and build release binary
COPY migration/ migration/
COPY src/ src/

ARG BUILD_SHA=dev
ENV BUILD_SHA=${BUILD_SHA}

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/connect/target \
    cargo build --release --bin connect-cli && \
    cp target/release/connect-cli /usr/local/bin/connect-cli

# =============================================================================
# Stage 2: Minimal runtime image
# =============================================================================
FROM debian:bookworm-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive
ENV LOCO_ENV=useradmin
ENV USERADMIN_PORT=3112
ENV ENABLE_INTERNAL_TLS=false

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    ffmpeg \
    libpq5 \
    libssl3 \
    libavutil57 \
    libavformat59 \
    libavcodec59 \
    libavdevice59 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -g 1000 -r openpilot && useradd -u 1000 -r -g openpilot -m openpilot \
    && mkdir -p /app /params && chown -R 1000:1000 /app /params

WORKDIR /app

COPY --from=builder /usr/local/bin/connect-cli /app/connect-cli
COPY --chown=1000:1000 assets/ /app/assets/
COPY --chown=1000:1000 config/ /app/config/

USER 1000:1000

EXPOSE 3112

ENTRYPOINT ["/app/connect-cli"]
CMD ["start", "--environment", "useradmin", "--server-and-worker"]
