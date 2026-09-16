# The Rill toolchain for Linux x86-64, built in Docker: the compiler from
# this checkout with LLVM 21, then packaged by tools/dist.sh into the
# tarball the downloads are. tools/dist-linux.sh runs it and copies the
# tarball out; the model editor (rillsdl) builds its image on that tarball.
#
#     docker build --target dist -f tools/Dockerfile -t rill-dist .

# ---- the compiler, built with LLVM 21 ---------------------------------------
FROM rust:1.89-bookworm AS build

# LLVM 21 from apt.llvm.org: inkwell links it statically into `rill`.
RUN apt-get update && apt-get install -y --no-install-recommends wget gnupg ca-certificates \
 && wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | gpg --dearmor -o /usr/share/keyrings/llvm.gpg \
 && echo "deb [signed-by=/usr/share/keyrings/llvm.gpg] http://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-21 main" > /etc/apt/sources.list.d/llvm.list \
 && apt-get update && apt-get install -y --no-install-recommends \
      llvm-21-dev libpolly-21-dev libzstd-dev zlib1g-dev libxml2-dev libtinfo-dev \
 && rm -rf /var/lib/apt/lists/*
# The checkout's .cargo/config.toml names the Homebrew LLVM; an environment
# variable set here wins over it.
ENV LLVM_SYS_211_PREFIX=/usr/lib/llvm-21

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p rill_cli -p rill_runtime

COPY lib ./lib

# ---- a packaged toolchain, for the downloads ------------------------------
# `docker build --target dist` and copy /src/dist/*.tar.gz out: the
# compiler and runtime with the wasm runtime, the standard library, the
# examples and the reference — tools/dist-linux.sh does the copying.
FROM build AS dist
RUN rustup target add wasm32-wasip1
COPY examples ./examples
COPY tools/dist.sh ./tools/dist.sh
COPY LANGUAGE.md README.md CHANGELOG.md ./
RUN bash tools/dist.sh
