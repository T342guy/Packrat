# Packrat builds to a single static binary with the frontend compiled in, so
# the runtime image is essentially just that binary.

FROM rust:1-alpine AS build
# rusqlite bundles SQLite, which means compiling C — hence the toolchain.
RUN apk add --no-cache build-base
WORKDIR /src

# Build the dependency graph on its own layer so editing the app doesn't
# recompile every crate.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
COPY static ./static
# Cargo skips a rebuild when only mtimes look stale, so nudge the entrypoint.
RUN touch src/main.rs && cargo build --release --locked


FROM alpine:3.21
LABEL org.opencontainers.image.title="Packrat" \
      org.opencontainers.image.description="Self-hosted inventory for garages, sheds and storage" \
      org.opencontainers.image.source="https://github.com/T342guy/packrat" \
      org.opencontainers.image.licenses="MIT"

# Unprivileged by default. The uid is fixed so a bind-mounted data directory
# can be chowned to match it from the host.
RUN adduser -D -H -u 10001 packrat \
    && mkdir -p /data \
    && chown packrat:packrat /data

COPY --from=build /src/target/release/packrat /usr/local/bin/packrat

USER packrat
VOLUME ["/data"]
EXPOSE 8080

ENV PACKRAT_DB=/data/inventory.db \
    PACKRAT_HOST=0.0.0.0 \
    PACKRAT_PORT=8080

# Touches the database, so a wedged file shows up as unhealthy rather than
# merely quiet.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -qO- http://127.0.0.1:8080/api/health || exit 1

# No CMD: arguments passed to `docker run` are appended as flags.
ENTRYPOINT ["packrat"]
