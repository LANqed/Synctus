# Build the Synctus relay server as a small container image.
#
# Two stages so the runtime image carries only the binary: the result is a few
# megabytes rather than the ~1.5 GB of the Rust toolchain.

FROM rust:alpine AS build

# musl-dev for the C runtime the linker needs; the relay itself has no C
# dependencies because rustls uses ring rather than OpenSSL.
RUN apk add --no-cache musl-dev

WORKDIR /src

# Copy manifests first so dependency compilation is cached independently of the
# source. `--locked` requires Cargo.lock to be committed.
COPY Cargo.toml Cargo.lock ./
COPY crates/core/Cargo.toml crates/core/
COPY crates/server/Cargo.toml crates/server/
COPY crates/desktop/Cargo.toml crates/desktop/
COPY crates/mobile/Cargo.toml crates/mobile/

# Stub sources so `cargo build` can resolve and compile dependencies. Only the
# server and its dependency (core) need to build here.
RUN mkdir -p crates/core/src crates/server/src crates/desktop/src crates/mobile/src \
    && echo "" > crates/core/src/lib.rs \
    && echo "fn main() {}" > crates/server/src/main.rs \
    && echo "fn main() {}" > crates/desktop/src/main.rs \
    && echo "" > crates/mobile/src/lib.rs \
    && cargo build --release -p synctus-server --locked \
    && rm -rf crates/*/src

COPY crates/ crates/

# Cargo decides what to rebuild from mtimes, and COPY can preserve timestamps
# older than the stub build above. Touching the sources forces the real compile.
RUN find crates -name '*.rs' -exec touch {} + \
    && cargo build --release -p synctus-server --locked

FROM alpine:3.21

RUN apk add --no-cache ca-certificates \
    && addgroup -S synctus \
    && adduser -S -G synctus synctus

COPY --from=build /src/target/release/synctus-server /usr/local/bin/synctus-server

USER synctus
EXPOSE 8787

ENV SYNCTUS_BIND=0.0.0.0:8787 \
    SYNCTUS_LOG=synctus_server=info

# The relay keeps no state on disk, so no volume is needed.
ENTRYPOINT ["/usr/local/bin/synctus-server"]
