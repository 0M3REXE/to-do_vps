# ------------------------------------------------------------------
# Multi-stage Dockerfile for the todo app
# Built in CI (GitHub Actions), never on the VPS.
# ------------------------------------------------------------------
# Stage 1 — build CSS + compile Rust binary
# ------------------------------------------------------------------
FROM rust:1.83-bookworm AS builder

# Install Node for Tailwind CSS build
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# 1a — cache Rust dependencies (only re-run when Cargo files change)
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && echo "" > src/lib.rs \
    && cargo fetch --locked \
    && rm -rf src

# 1b — build Tailwind CSS
COPY package.json package-lock.json ./
RUN npm ci

COPY static/ static/
COPY templates/ templates/
COPY src/ src/
RUN npm run css:build

# 1c — compile release binary
COPY migrations/ migrations/
RUN cargo build --release --locked --bin todo \
    && strip target/release/todo \
    && test -s target/release/todo

# ------------------------------------------------------------------
# Stage 2 — minimal runtime image
# ------------------------------------------------------------------
FROM debian:bookworm-slim

# TLS root certificates for libSQL remote connections + curl for healthz
RUN apt-get update -qq \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Non-root user
RUN groupadd --gid 10001 app \
    && useradd --uid 10001 --gid app --shell /usr/sbin/nologin --create-home app

COPY --from=builder /build/target/release/todo /usr/local/bin/todo

# Data volume mount point
RUN mkdir -p /app/data && chown app:app /app/data
VOLUME ["/app/data"]

USER app
WORKDIR /app

ENV BIND_ADDRESS=0.0.0.0
ENV PORT=3000
ENV DATA_ROOT=/app/data

EXPOSE 3000

HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:${PORT}/healthz || exit 1

ENTRYPOINT ["/usr/local/bin/todo"]
