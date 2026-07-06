# Infrastructure

The deployment and runtime topology for Second Brain — Docker Compose, the public-facing service, and the deploy pipeline. Personal-Scale: a single VPS, two long-running services.

## Language

**Edge**:
The single public-facing service in the two-service topology — terminates TLS (Caddy, auto-HTTPS), serves the baked-in PWA Bundle via `file_server`, and reverse-proxies `/api/*` to the Backend. Disposable as an *image* — rebuilt as a custom image per deploy — but NOT stateless at runtime: Caddy's ACME state (issued Let's Encrypt certificates and ACME account keys in `/data`, plus autosaved config in `/config`) must survive container recreations, so both paths are mounted on named volumes (`caddy_data`, `caddy_config`). Without `/data`, every `docker compose down/up` or deploy wipes ACME state and forces a fresh cert request, tripping Let's Encrypt's 5-issuances-per-168h rate limit and taking HTTPS down (#55). The ACME account key lives in this volume — same host-disk trust boundary as the Brain File, not the Zero-Trust Image boundary (ADR-0004).
_Avoid_: proxy, gateway, reverse proxy, frontend host, front door

**PWA Bundle**:
The build output of the frontend (SvelteKit `adapter-static`), baked into the Edge image at image-build time rather than mounted from a shared volume. The frontend context's entire output as seen from infrastructure — a static artifact, not a running service.
_Avoid_: static files, build folder, dist, frontend image, static assets

**Brain File**:
The single SQLite database file holding the entire persistent state of the Second Brain — every user's braindumps, concepts, edges, provenance entries, type-history entries, and embeddings (sqlite-vec, in-process with the graph, partitioned by `user_id`). One file, many per-user brains; the load-bearing artifact the rest of the infrastructure orbits. Lives on a named Docker volume mounted into the backend container; never bind-mounted, never baked into an image.
_Avoid_: the database, the DB, sqlite file, data volume, datastore

**Brain Replica**:
A continuous WAL-streamed copy of the Brain File in offsite object storage, maintained by a sidecar process running alongside the backend. Distinct from a periodic snapshot: replication is second-by-second, so the Recovery Point Objective is seconds, not hours — the epistemic trust contract of a Second Brain forbids losing a day's worth of braindumps to a VPS fire. Lives in a failure domain fully isolated from the primary (off-provider object storage).
_Avoid_: backup, snapshot, offsite copy, replica, WAL archive

**Zero-Trust Image**:
A container image that contains no secrets of any kind — no API key, no R2 credential, no session secret ever touches a Dockerfile `ENV` or `COPY` directive. Images are treated as public-safe artifacts: if a Zero-Trust Image were published to the public internet tomorrow, the only thing leaked is compiled code. The constraint runs in both directions — it forbids baking secrets "just for convenience," and it guarantees that image registry leakage is a code leak, not an infrastructure breach.
_Avoid_: clean image, generic image, secret-free image

**Health Push**:
A push-based infrastructure alert fired by a host-cron health check — distinct from the frontend admin tab's pull-based log view. Fires when the Brain Replica's replication lag exceeds a threshold or the Brain File's volume nears capacity: the one failure class a pull model cannot catch, because silent replication drift only surfaces when you open the app, by which point the trust contract from ADR-0002 has already been broken. Shipped to a push-notification endpoint (ntfy.sh or equivalent) so the alert finds the user, not the reverse. Zero RAM, zero containers — one cron entry and one webhook.
_Avoid_: monitoring, alerting, uptime check, health check dashboard

**Deploy State**:
The non-secret deployment state GHA is authorized to write to the VPS — image tags (SHA-pinned), version identifiers — kept in a separate `deploy.env` file sourced by Compose alongside the secret `.env`. The separation is the complement of ADR-0004's Zero-Trust Image rule: GHA is blind to secrets (the `.env` file, manually placed, never touched by CI) but may write Deploy State (the `deploy.env` file, written on every deploy via SSH). The two-file split keeps a compromised GitHub account from leaking runtime credentials while still letting CI control which image the VPS runs. `deploy.sh` additionally syncs the non-secret infra config (docker-compose.yml, litestream.yml, health-push.sh) from the public repo at the deployed SHA (ADR-0010) — GHA still writes only `deploy.env`; the config is fetched by `deploy.sh` itself, so a leaked SSH key can replay past configs but never craft novel ones (ADR-0003 invariant preserved).
_Avoid_: deploy config, image tags file, CI state, deployment config
