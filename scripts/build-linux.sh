#!/bin/bash
# Builds Ranger for Ubuntu x86_64 (.deb and .AppImage) inside a Docker container.
# Works from macOS through OrbStack or Docker Desktop (amd64 is emulated).
# Output: dist-linux/
set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p dist-linux

docker run --rm --platform linux/amd64 \
  -v "$PWD":/src:ro \
  -v "$PWD/dist-linux":/out \
  -v ranger-linux-cargo:/root/.cargo \
  -v ranger-linux-target:/build-target \
  -e CARGO_TARGET_DIR=/build-target \
  -e APPIMAGE_EXTRACT_AND_RUN=1 \
  ubuntu:22.04 bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq curl ca-certificates build-essential pkg-config file xdg-utils \
      libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev >/dev/null

    if ! command -v node >/dev/null; then
      curl -fsSL https://deb.nodesource.com/setup_22.x | bash - >/dev/null
      apt-get install -y -qq nodejs >/dev/null
    fi
    if [ ! -x /root/.cargo/bin/cargo ]; then
      curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
    fi
    . /root/.cargo/env

    # Fresh copy without the macOS node_modules and build output
    mkdir -p /build && cd /src
    tar --exclude=./node_modules --exclude=./src-tauri/target --exclude=./dist --exclude=./dist-linux --exclude=./.hallmark -cf - . | tar -xf - -C /build
    cd /build
    npm ci --no-audit --no-fund

    echo "== Rust tests on Linux"
    (cd src-tauri && cargo test --lib 2>&1 | grep -E "^test |test result")

    echo "== Bundling"
    npx tauri build --bundles deb,appimage 2>&1 | grep -E "Finished|Bundling|Error|error" || true

    cp -v /build-target/release/bundle/deb/*.deb /build-target/release/bundle/appimage/*.AppImage /out/
  '
ls -lh dist-linux
