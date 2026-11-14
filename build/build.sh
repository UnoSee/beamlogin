#!/bin/sh
# Build release binaries for every platform into dist/.
# Run inside the image from build/Dockerfile (see README "Building").
set -eu
cd /src
mkdir -p dist
build() { # target, output name, builder
  echo "==> $1"
  cargo $3 --release --target "$1"
  ext=""; case "$1" in *windows*) ext=".exe";; esac
  cp "target/$1/release/beamlogin$ext" "dist/$2"
}
build x86_64-unknown-linux-musl   beamlogin-x86_64-linux       zigbuild
build aarch64-unknown-linux-musl  beamlogin-aarch64-linux      zigbuild
build x86_64-pc-windows-gnu       beamlogin-x86_64-windows.exe build
build x86_64-apple-darwin         beamlogin-x86_64-macos       zigbuild
build aarch64-apple-darwin        beamlogin-aarch64-macos      zigbuild
ls -l dist
