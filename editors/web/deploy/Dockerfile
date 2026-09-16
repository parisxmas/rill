# The model editor, published: the compiler built from this checkout with
# LLVM 21, the server built with it, and a small image that carries the
# two, the standard library, a C linker for the programs the server
# builds, and nothing else. Programs run sandboxed, since the server is
# opened to the network here (`--host 0.0.0.0`): see `--allow` below.
#
#     docker build -f editors/web/deploy/Dockerfile -t rillsdl .
#     docker run -p 127.0.0.1:8765:8765 -v rillsdl-models:/models --read-only --tmpfs /tmp:size=512m,exec rillsdl
#
# Built from the repository root, so the context is the whole checkout
# (less what .dockerignore leaves out).

# ---- stage 1: the compiler and the server ---------------------------------
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
COPY editors/web ./editors/web
RUN mkdir -p /out \
 && RILL_PATH=/src/lib ./target/release/rill build editors/web/server.rill -o /out/server

# ---- a packaged toolchain, for the downloads ------------------------------
# `docker build --target dist` and copy /src/dist/*.tar.gz out: the same
# compiler and runtime as the server's, with the wasm runtime, the
# standard library, the examples and the reference — tools/dist-linux.sh
# does the copying.
FROM build AS dist
RUN rustup target add wasm32-wasip1
COPY examples ./examples
COPY tools/dist.sh ./tools/dist.sh
COPY LANGUAGE.md README.md CHANGELOG.md ./
RUN bash tools/dist.sh

# ---- stage 2: what runs ---------------------------------------------------
FROM debian:bookworm-slim

# `cc` links every program the server builds; perl bounds a run's time;
# the rest is what the statically linked LLVM still asks the system for.
RUN apt-get update && apt-get install -y --no-install-recommends \
      gcc libc6-dev perl-base zlib1g libzstd1 libxml2 libtinfo6 \
 && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 1000 rill
WORKDIR /app
COPY --from=build /src/target/release/rill /src/target/release/librill_runtime.a /app/bin/
COPY --from=build /out/server /app/bin/server
COPY --from=build /src/lib /app/lib
COPY --from=build /src/editors/web/static /app/editors/web/static
COPY examples /app/examples
COPY editors/web/deploy/entrypoint.sh /app/bin/entrypoint.sh
RUN chmod +x /app/bin/entrypoint.sh && mkdir -p /models && chown rill:rill /models

ENV RILL_PATH=/app/lib
USER rill
EXPOSE 8765
VOLUME ["/models"]

# The models are the volume; the page and the compiler are the image.
# `--allow` names what a program may reach past the process (files, net,
# ffi, os — or `all` to trust everyone who can reach the server); left
# empty, nothing.
ENTRYPOINT ["/app/bin/entrypoint.sh"]
CMD ["--allow", ""]
