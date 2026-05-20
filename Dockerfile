# syntax=docker/dockerfile:1.7
#
# flutter-cli — containerized build.
#
# The image is split in two stages so we don't carry the Rust toolchain
# (~1 GB) and the workspace target/ directory into the final layer.
#
#   1. `builder` — Debian-based Rust image, builds `flutter-cli` against
#      the pinned toolchain from rust-toolchain.toml. Cargo and target
#      caches are mounted so re-builds inside the same buildx context
#      are incremental.
#   2. `runtime` — Debian slim with just the static-ish binary plus the
#      external tools `flutter-cli` shells out to (git, adb, OpenJDK).
#      The Flutter SDK itself is NOT baked in — it changes too often,
#      is too large to redistribute, and most users want to mount their
#      own. Mount it at runtime with `-v $FLUTTER_ROOT:/opt/flutter:ro`
#      and export `PATH=/opt/flutter/bin:$PATH`.
#
# Build:
#   docker build -t flutter-cli:dev .
#
# Run (just the binary, no Flutter SDK):
#   docker run --rm -it flutter-cli:dev --help
#
# Run against a host Flutter project (Linux/macOS host):
#   docker run --rm -it \
#     -v "$PWD":/work -w /work \
#     -v "$FLUTTER_ROOT":/opt/flutter:ro \
#     -e PATH=/opt/flutter/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
#     flutter-cli:dev run --basic
#
# Note: iOS device interaction (xcrun, idevicepair) only works on macOS;
# the container is Linux. Android-over-USB needs `--device /dev/bus/usb`
# and adequate udev rules on the host.

# ---------- builder ----------
ARG RUST_VERSION=1.82
FROM rust:${RUST_VERSION}-bookworm AS builder

WORKDIR /src

# Install build deps. `pkg-config` and `libssl-dev` aren't strictly
# required today (no openssl-sys in the dep graph) but they're cheap
# insurance for future deps without forcing a rebase.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config libssl-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Copy manifests first so the dependency-fetch layer is cacheable even
# when only source files change. `cargo fetch` warms the registry/git
# caches against Cargo.lock so the real build is offline-able.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates

# Build with BuildKit cache mounts so re-builds inside the same buildx
# context don't re-download crates or re-link from scratch.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin flutter-cli \
 && cp /src/target/release/flutter-cli /usr/local/bin/flutter-cli \
 && strip /usr/local/bin/flutter-cli

# ---------- runtime ----------
FROM debian:bookworm-slim AS runtime

# - `git` and `curl` are dependencies of `flutter` itself (used by
#   `flutter pub get`, version detection, etc.); even though we don't
#   bundle the SDK, mounting an external one still requires them.
# - `adb` (in `android-tools-adb`) lets `flutter-cli` talk to Android
#   devices forwarded via `--device /dev/bus/usb`.
# - `default-jdk-headless` because the Android toolchain pulls it in
#   transitively; without it `flutter build apk` aborts on first run.
# - `ca-certificates` so HTTPS calls from inside the container resolve.
# - `unzip`/`xz-utils` so a mounted Flutter SDK's bootstrap scripts work
#   when they need to extract their cache.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git unzip xz-utils \
      android-tools-adb \
      default-jdk-headless \
 && rm -rf /var/lib/apt/lists/*

# Non-root user so files written from `flutter pub get` / `flutter
# build` land with a sensible owner. UID 1000 matches the default Linux
# desktop user — mount-friendly on most hosts.
ARG APP_UID=1000
ARG APP_GID=1000
RUN groupadd --gid ${APP_GID} app \
 && useradd  --uid ${APP_UID} --gid app --create-home --shell /bin/bash app

COPY --from=builder /usr/local/bin/flutter-cli /usr/local/bin/flutter-cli

# Give the app user a writable cache dir for cargo-style tools that
# probe XDG_CACHE_HOME; flutter-cli itself stores nothing here today
# but the Flutter SDK absolutely will when mounted.
ENV XDG_CACHE_HOME=/home/app/.cache \
    XDG_CONFIG_HOME=/home/app/.config \
    XDG_DATA_HOME=/home/app/.local/share

USER app
WORKDIR /work

ENTRYPOINT ["/usr/local/bin/flutter-cli"]
CMD ["--help"]
