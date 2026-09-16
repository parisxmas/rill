#!/bin/bash
# The Linux x86_64 toolchain tarball, built in Docker from this checkout
# — on a Linux box natively, or on a Mac under emulation (slow) — and
# copied into dist/.
#
#     bash tools/dist-linux.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
docker build --platform linux/amd64 --target dist -f tools/Dockerfile -t rill-dist .
id=$(docker create --platform linux/amd64 rill-dist)
mkdir -p dist
docker cp "$id:/src/dist/." dist/linux-tmp
docker rm "$id" >/dev/null
mv dist/linux-tmp/*.tar.gz dist/
rm -rf dist/linux-tmp
ls -la dist/*.tar.gz
