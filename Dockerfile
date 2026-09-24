# syntax=docker/dockerfile:1

# Base images are pinned by digest so a rebuild produces the same image
# rather than whatever the tag points at that day. They are written out in
# the FROM lines rather than as ARGs so Dependabot can see and update them.

# ---- tools ---------------------------------------------------------------
# Built from source with --locked rather than downloaded as a binary, and in
# its own stage so a change to GhostDock's source does not rebuild it.
FROM docker.io/library/rust:1.98.1-trixie@sha256:a8a5f0a1e5fe7dfe1d352591e4a1c7dd2c08fd70475cae872cf3458ba0df0546 AS tools
# brotli: the web build writes precompressed assets (crates/web/precompress.sh).
# hadolint ignore=DL3008
RUN apt-get update \
 && apt-get install -y --no-install-recommends brotli \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add wasm32-unknown-unknown \
 && cargo install --locked trunk@0.21.14

# ---- build ---------------------------------------------------------------
FROM tools AS build
WORKDIR /src
COPY . .

# The web client first: it is architecture-independent wasm, and trunk
# fetches its own pinned wasm-bindgen and wasm-opt.
WORKDIR /src/crates/web
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    trunk build --release \
 && cp -r /src/crates/web/dist /web

WORKDIR /src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p server \
 && cp /src/target/release/ghostdock /usr/local/bin/ghostdock

# ---- runtime -------------------------------------------------------------
# Not distroless: GhostDock shells out to the official `docker compose` and
# `git`, which is the whole basis of its fidelity guarantee.
FROM docker.io/library/debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a AS runtime

# DL3008 (pin apt package versions) is deliberately not followed. Debian
# removes superseded versions from its archive when a security update lands,
# so an exact pin makes this build fail on the very next fix rather than pick
# it up. Reproducibility comes from the digest-pinned base image instead, and
# `just security-image` scans what is actually installed.
# Review: 2027-03-01, or on moving to a Debian release with snapshot pinning.
# hadolint ignore=DL3008
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl gnupg git tini \
 && install -m 0755 -d /etc/apt/keyrings \
 && curl -fsSL https://download.docker.com/linux/debian/gpg \
      -o /etc/apt/keyrings/docker.asc \
 && chmod a+r /etc/apt/keyrings/docker.asc \
 && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] \
      https://download.docker.com/linux/debian trixie stable" \
      > /etc/apt/sources.list.d/docker.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends docker-ce-cli docker-compose-plugin \
 && apt-get purge -y gnupg curl \
 && apt-get autoremove -y \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/ghostdock /usr/local/bin/ghostdock
COPY --from=build /web /usr/share/ghostdock/web

ENV GHOSTDOCK_DATA_DIR=/var/lib/ghostdock \
    GHOSTDOCK_BIND=0.0.0.0:8080 \
    GHOSTDOCK_UI_DIR=/usr/share/ghostdock/web

# PATH CONTRACT: mount the data directory at the SAME absolute path as on
# the host, e.g. `-v /var/lib/ghostdock:/var/lib/ghostdock`. The compose CLI in this
# container resolves relative paths in a stack's file against it, while the
# daemon on the host interprets the result; the two must agree. GhostDock checks
# this at startup and says so on its front page when it does not hold.
#
# Deliberately no VOLUME instruction. It creates an anonymous volume on every
# start -- even when the data directory is mounted somewhere else, which is
# the recommended setup for a custom path -- so each restart leaked one. It
# also hid the mistake of mounting nothing, by quietly putting the data in a
# volume that is orphaned the next time the container is replaced.
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD ["/usr/local/bin/ghostdock", "healthcheck"]

# Runs as root. Access to the Docker socket is root on the host regardless
# of the user inside this container, so a non-root user here would be
# theatre rather than a boundary -- and would need the socket's group id,
# which differs from host to host.
#
# tini as PID 1 reaps anything the compose or git children leave behind.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/ghostdock"]
