# Cross-compile toolchain for every Beamlogin release target.
FROM rust:1.97-bookworm
RUN apt-get update \
 && apt-get install -y --no-install-recommends mingw-w64 python3-pip musl-tools \
 && rm -rf /var/lib/apt/lists/*
RUN pip3 install --break-system-packages ziglang==0.13.0 \
 && cargo install --locked cargo-zigbuild
RUN rustup target add \
      x86_64-unknown-linux-musl aarch64-unknown-linux-musl \
      x86_64-pc-windows-gnu \
      x86_64-apple-darwin aarch64-apple-darwin
WORKDIR /src
