# --- build stage -------------------------------------------------------------
FROM rust:1.96-bookworm AS build
WORKDIR /app
# sqlite-vec and bundled SQLite compile C; we use rustls (no system SSL).
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# Cache deps first: copy manifests and pre-build, then copy source.
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY tests ./tests
RUN cargo build --release

# --- runtime stage -----------------------------------------------------------
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app/target/release/backend /app/backend
ENV HOST=0.0.0.0 \
    PORT=8080 \
    DATABASE_URL=/data/second_brain.db \
    LOG_FORMAT=json \
    RUST_LOG=info,second_brain_backend=debug
RUN mkdir -p /data
VOLUME ["/data"]
EXPOSE 8080
CMD ["/app/backend"]
