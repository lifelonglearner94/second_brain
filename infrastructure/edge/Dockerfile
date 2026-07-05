# Edge image (ADR-0001): a custom Caddy image that terminates TLS, serves the
# baked-in PWA Bundle via file_server, and reverse-proxies /api/* to the
# internal-only Backend (ADR-0006).
#
# Zero-Trust Image (ADR-0004): no secret of any kind ever touches a COPY or
# ENV directive. Publish this image to the public internet and the only thing
# leaked is compiled code + static assets — zero credentials.

# --- PWA Bundle stage --------------------------------------------------------
# Build the SvelteKit adapter-static output (frontend/build/) at image-build
# time. The PWA Bundle is baked in, not mounted from a shared volume, so the
# Edge stays stateless and disposable (ADR-0001).
FROM node:22-bookworm-slim AS bundle
WORKDIR /app/frontend
# Cache npm deps across source changes.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

# --- Edge stage --------------------------------------------------------------
FROM caddy:2-alpine AS edge
# The Caddyfile is configuration, not state — bake it in so the Edge remains a
# disposable image rebuilt per deploy. The CADDYFILE build arg selects which
# Caddyfile to bake in: the default (Caddyfile) binds :80 with no TLS for local
# dev; CI passes CADDYFILE=Caddyfile.prod to get auto-HTTPS on the real domain.
# Zero-Trust (ADR-0004): neither Caddyfile contains a secret — the domain is
# public config, not a credential.
ARG CADDYFILE=Caddyfile
COPY infrastructure/edge/${CADDYFILE} /etc/caddy/Caddyfile
# Bake the PWA Bundle into the serve root.
COPY --from=bundle /app/frontend/build /srv
EXPOSE 80 443
