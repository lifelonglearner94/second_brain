# Second Brain

A single-user, voice-first **Progressive Web App** that turns your stream of thoughts into a living knowledge graph — and lets you chat with your own mind.

Speak or type a thought (a *braindump*); a hosted LLM extracts *concepts* and typed *edges* out of it, the graph accretes over time, and you navigate it as an interactive 3D/2D force-directed visualization or query it through grounded, cited chat. The graph is load-bearing; vectors are seed and backfill. Nothing the LLM deduces enters the graph silently — every inference is a human-gated proposal.

> **Personal-Scale, Single-User.** One brain, one user, one 8 GB VPS. The entire persistent state of the system is a single SQLite file — the **Brain File** — streamed second-by-second to offsite object storage. The architecture is deliberately sized to one human's decade of thinking, not to a fleet.

---

## At a glance

| Layer | Technology |
|---|---|
| **Frontend** | SvelteKit (Svelte 5, runes) + `adapter-static` PWA; `3d-force-graph`/Three.js (3D) with `sigma.js` (2D) fallback; `graphology` + ForceAtlas2 + Louvain; WebAuthn passkeys; Deepgram Nova-3 STT (Web Speech API offline fallback) |
| **Backend** | Rust (edition 2021, MSRV 1.82) + Axum 0.8; `petgraph` (in-memory) + `rusqlite` (ACID); `sqlite-vec` (in-process vectors); `webauthn-rs` |
| **AI** | Hosted **Gemini** for LLM (clean / extract / synthesize) and embeddings — no self-hosted models. One `Llm` trait seam; `FakeLlm` stands in for dev/CI |
| **Infra** | Docker Compose: **Edge** (custom Caddy image: TLS, baked PWA, `/api` reverse-proxy) + **Backend** (Rust) + **Litestream** sidecar (WAL → Cloudflare R2) |
| **CI/CD** | GitHub Actions → build on runners → push to GHCR → VPS pulls via command-restricted SSH key. SHA-pinned image tags, 30-second rollback |
| **VPS** | Single Debian box (4 GB RAM + 4 GB swap); nftables firewall; host-cron Health Push to ntfy |

---

## How it fits together

```
                        ┌──────────────────────────────────────────────┐
                        │                     VPS                      │
   browser (PWA) ──HTTPS──▶  Edge (Caddy)  ──/api/*──▶  Backend (Axum)  │
   voice · graph · chat     · TLS + file_server     · Rust orchestrator │
                            · baked PWA Bundle      · petgraph + SQLite │
                            · reverse-proxy /api    · sqlite-vec (KNN)  │
                                    │                       │  Gemini API
                                    │                       ▼  (LLM + embed)
                                    │              Litestream sidecar ──WAL──▶ Cloudflare R2
                                    │              (Brain Replica, RPO ~1s)
                                    └────────────── sqlite_data volume ──┘
                                                      (the Brain File)
```

- **Frontend → Backend**: the PWA is *strictly a view* over the backend. It calls the backend over HTTP to submit braindumps, read the graph, run chat/retrieval, and arbitrate human-in-the-loop proposals. The backend owns the data model; the frontend never computes anything whose output is consumed by anything other than the display ([frontend ADR-0001][fe-0001]).
- **Three read surfaces**: Retrieval (seed-then-expand, [backend ADR-0004][be-0004]), Chat (grounded synthesis, [backend ADR-0005][be-0005]), and the Thematic Read Model (backend-owned Louvain partition, [backend ADR-0008][be-0008]).
- **Two write surfaces**: braindump ingest (`POST /braindumps`) and governed chat write-back (`POST /chat/inferences*`), both routing through the same fractal governance (concepts >95% auto-merge, types >99.5%, inferences always human-gated).

See [`CONTEXT-MAP.md`](./CONTEXT-MAP.md) for the canonical context map, and each context's `CONTEXT.md` for its controlled vocabulary.

---

## The domain language (short form)

The repo maintains a **controlled vocabulary** per context to prevent synonym drift. The short version:

- **Braindump** — the atomic unit of input: an immutable snapshot of your state of mind at a timestamp. Voice-transcribed or typed, confirmed at explicit submit, preserved verbatim (with an LLM-cleaned rendering shown by default). Edits exist only for error-correction (STT typos); substantive evolution spawns a *new* braindump and the graph grows additively ([backend ADR-0007][be-0007]).
- **Concept** — a node in the graph, extracted from one or more braindumps. Recurs and accretes over time. Identity is by **embedding similarity**, not label ([backend ADR-0001][be-0001]).
- **Edge** — a typed, directional connection `A —[type]→ B`. The type is drawn from the **Ontology** and never invented by the LLM. One edge per `(source, original type, target)` **accretes provenance** and survives until its last asserter is removed ([backend ADR-0002][be-0002]).
- **Provenance** — the origin-tagged list of assertions backing an edge or concept: a human braindump, a structural inference, or a thematic inference. Lets you distinguish raw thoughts from endorsed LLM deductions, and purge the latter if the brain drifts ([backend ADR-0006][be-0006]).
- **Ontology** — a governed, evolving vocabulary of edge types. New types enter through a proposal-and-approval process; refactors run async at Temperature=0 against a pinned model and retag via an append-only **type history** ([backend ADR-0003][be-0003]).
- **Retrieval** — concept-embeddings seed entry concepts, typed edges expand the neighbourhood, braindumps from the subgraph (plus braindump-embedding backfill) form the context for chat.
- **Chat** — grounded synthesis over retrieved braindumps with **mandatory citations**, graph-constrained inference, and **silence when unsupported** ("I cannot find graph-supported evidence to answer this"). Chat may also *propose* inferences as new edges, routed through the Endorsement Queue.
- **Chat Inference** — a connection chat proposes (structural = graph-backed path summary; thematic = statistical hypothesis from an ephemeral Louvain cluster). Always a proposal; never auto-endorsed. Thematic proposals carry a frozen **Thematic Snapshot** as an audit trail ([backend ADR-0009][be-0009]).

Full definitions (and the "Avoid:" synonym lists) live in [`backend/CONTEXT.md`](./backend/CONTEXT.md), [`frontend/CONTEXT.md`](./frontend/CONTEXT.md), and [`infrastructure/CONTEXT.md`](./infrastructure/CONTEXT.md).

---

## Backend — the Rust orchestrator & graph engine

`backend/` — a single-binary, single-connection Rust/Axum app where the typed-edge knowledge graph is load-bearing and the LLM is wired behind one trait seam.

**Storage.** One `Arc<Mutex<Connection>>` SQLite database in **WAL** mode with `foreign_keys=ON`, `busy_timeout=5000`. `sqlite-vec` is registered as a process-global auto-extension; the three `vec0` virtual tables (`concept_embeddings`, `braindump_embeddings`, `type_embeddings`, cosine distance) live *in the same DB* as the graph. Single-connection is intentional: it makes the extraction + embedding + identity-resolution accretion a single `BEGIN … COMMIT` against in-process `sqlite-vec` ([backend ADR-0001][be-0001]) — no external vector server.

**Migrations** are forward-only, additive, idempotent `CREATE TABLE IF NOT EXISTS` blocks. Tables: `passkeys`, `sessions`, `braindumps`, `ontology` (seeded with 13 day-zero edge types), `concepts` + `concept_provenance`, `edges` + `edge_provenance` + `edge_type_history` + `merge_suggestions`, `type_proposals`, `graph_tombstones`, `thematic_snapshots`, `chat_inference_proposals` + `edge_inference_provenance`.

**The `GraphRepo` seam** ([`backend/src/graph_repo.rs`](./backend/src/graph_repo.rs)) is the central abstraction — *every* read and write against the knowledge graph goes through this trait, so call sites depend on the interface, not the storage adapter. Production wires `SqliteGraphRepo`; tests wire `InMemoryGraphRepo` (HashMap-backed, hermetic). Domain modules are thin HTTP adapters + delegating wrappers.

**The `Llm` seam** ([`backend/src/llm.rs`](./backend/src/llm.rs)) — one trait serves all five LLM/embedding roles: `clean`, `generate_pinned` (Temperature=0, ADR-0003), `synthesize` (ADR-0005), `extract` (structured output), `embed_document`/`embed_query`. `GeminiClient` ([`backend/src/gemini.rs`](./backend/src/gemini.rs)) is the real implementation; `FakeLlm` is the deterministic test stand-in. `GeminiClient::from_env()` returns `Ok(None)` when `GEMINI_API_KEY` is unset, so dev/CI runs without a key.

### HTTP API surface (26 routes)

Public: `GET /health`, `GET /ontology`, `POST /auth/{register,login}/{begin,finish}`, `POST /auth/recover` (stub).
Protected (session cookie): `GET /me`, `POST /auth/logout`, `POST|GET|PATCH|DELETE /braindumps[/:id]`, `GET|POST /merge-suggestions[/:id/{approve,reject}]`, `POST /retrieve`, `POST /chat`, `GET|POST /chat/inferences[/thematic|/:id/{endorse,reject}]`, `GET /thematic`, `GET /graph` (gzipped Global Topology Snapshot), `GET /graph/delta?since=`, `GET /admin/logs`, `POST /ontology/propose`, `GET /ontology/proposals`, `POST /ontology/proposals/:id/{approve,reject}`.

Every non-auth handler is a thin HTTP adapter — the pipeline logic lives in domain modules so it's unit-testable without an HTTP roundtrip.

### Auth

WebAuthn **passkey** (primary) via `webauthn-rs`, with an opaque session cookie — **no JWT in `localStorage`** (rejected as an XSS anti-pattern). The server mints a ≥256-bit opaque session id stored in a SQLite row, set as `__Host-sb_session` `httpOnly; Secure; SameSite=Strict`. A deploy-time **singleton lock** closes registration once one passkey exists (one user, one passkey). `require_session` middleware guards protected routes.

### Ingest → extraction → accretion pipeline

1. `POST /braindumps` validates non-empty, then `braindump::ingest`: **clean** (LLM) → `insert_braindump` → `accrete`.
2. `accrete`: read ontology slugs → `llm.extract` (structured JSON constrained to the governed vocabulary) → `graph::ingest_extraction`.
3. `ingest_extraction`: embed the braindump + each concept label (LLM network calls), then delegate the **atomic accretion** to `GraphRepo::ingest_extraction` — identity resolution (concept-embedding KNN ≥0.95 auto-merge, 0.80–0.95 merge-suggestion, else new), provenance recording, append-only type history, and embedding storage all commit in one transaction.
4. Edits (`PATCH`) overwrite the verbatim in place (id + `created_at` untouched), re-clean, re-extract, re-accrete with stale extraction retracted first. Deletes cascade through the graph (drop from provenance; concepts/edges vanish when their last asserter is removed; orphaned endpoint-cascade edges are deleted).

---

## Frontend — the PWA UI

`frontend/` — a SvelteKit (Svelte 5, runes) + `adapter-static` PWA, *strictly a view* over the backend with exactly one named write-intent exception ([frontend ADR-0005][fe-0005]). The built static bundle is baked into the Edge image at image-build time.

### Screens & user flow

1. **`/`** — landing page with a live `GET /health` status (db + sqlite-vec).
2. **`/login`** — passkey-only auth (register / sign in / recover). Uses `@simplewebauthn/browser`.
3. **`/app`** — the main screen, the **Spatial View-Graph**. Loads the **Global Topology Snapshot** (`GET /graph`, gzipped) network-first with an IndexedDB fallback (the **Frozen Graph** offline mode). Renderer is capability-detected: 3D `3d-force-graph` + Three.js with bloom postprocessing, or 2D `sigma.js` fallback for iOS / weak mobile GPUs. ForceAtlas2 layout runs locally each session (ephemeral); Louvain partition IDs come from the backend and drive cluster colors + z-layering. Camera + selected node persist to LocalStorage (**Viewport State**). **Delta Sync** pulls `GET /graph/delta` on window focus and after each ingest.
4. **Active Capture** — the voice/text input at the top of `/app`. Accumulates streaming Deepgram Nova-3 (online, `de`) or Web Speech API (offline) STT plus manual keystrokes. **Explicit submit** is the gate to becoming a braindump. Offline or Web-Speech submissions route to **Pending Captures** for review-and-confirm (offline STT is significantly less accurate).
5. **`/app/chat`** — query → `POST /chat` → answer with `[bd:<id>]` citation chips (traversal paths hidden) or **Explicit Silence**. Clicking a chip opens the **Document Modal** (cleaned by default, verbatim behind toggle; the edit flow populates with the *verbatim*, never the cleaned — [frontend ADR-0003][fe-0003]).
6. **`/app/housekeeping`** — low-epistemic-weight HITL queue: concept/type merges the system flagged as borderline. Action verb: **"Merge."**
7. **`/app/endorsements`** — high-epistemic-weight HITL queue: chat-inferred edges (structural & thematic). Action verb: **"Approve Connection."** Each proposal discloses its evidence — a traversable path (structural) or a frozen Thematic Snapshot (thematic). Endorsed edges merge optimistically into the Spatial View-Graph.
8. **`/app/pending`** — review-and-confirm offline captures.
9. **`/app/admin/logs`** — structured log viewer (hidden; revealed by a 5-tap gesture on the title), backed by `GET /admin/logs`.

### Offline / PWA architecture

A deliberately **dumb service worker** caches only the app shell and never intercepts `/api/*` ([frontend ADR-0005][fe-0005]). All offline business logic lives in the testable application layer. Persistent state is exactly three named items:

- **IndexedDB** — `topology-snapshot` (read cache for the Frozen Graph) and `pending-captures` (the sole **write-intent** exception to "strictly a view").
- **LocalStorage** — `sb.viewport-state` (camera position + selected node).

### Key client libraries

`src/lib/api/client.ts` (the typed API client, `credentials: 'include'`), `src/lib/graph/` (graphology → `3d-force-graph`/`sigma` projection, ForceAtlas2, delta/merge application, capability detection), `src/lib/capture/` (Deepgram + Web Speech STT, the explicit-submit gate), `src/lib/chat/` (citation parsing, Document Modal), `src/lib/state/` (Svelte 5 runes singletons: `GraphStore`, `session`, `pending-captures`, `housekeeping`, `endorsement-queue`, `admin-logs`, `online`).

---

## Infrastructure — deployment & runtime topology

`infrastructure/` + root `docker-compose.yml` — a **two-service + one-sidecar** Compose topology on a single VPS ([infra ADR-0001][in-0001]).

| Service | Role |
|---|---|
| **Edge** | Custom Caddy image: terminates TLS (auto-HTTPS in prod), serves the baked PWA Bundle via `file_server`, reverse-proxies `/api/*` → `http://backend:8080`. Stateless, disposable. |
| **Backend** | Rust/Axum, internal-only ([infra ADR-0006][in-0006]) — `expose: ["8080"]` with *deliberately no* `ports:` block. Holds the Brain File on the named volume `sqlite_data:/data`. |
| **Litestream** | Infra sidecar ([infra ADR-0002][in-0002]): streams the Brain File WAL second-by-second to Cloudflare R2 (RPO ~1s). Metrics on `127.0.0.1:9090` (loopback only). |

### Secrets & the Zero-Trust Image contract

Runtime secrets live in a single `infrastructure/.env`, placed **manually once** over SSH and injected via `env_file` — **GHA is completely blind to runtime secrets** ([infra ADR-0004][in-0004]). The complement is the **Zero-Trust Image** rule ([infra ADR-0009][in-0009]): no secret ever touches a Dockerfile `ENV`/`ARG`/`COPY`, enforced by a CI guard (`infrastructure/test/zero-trust.sh`). The committed `infrastructure/.env.example` is the single source of truth for the secret key list (values blank, `[SECRET]`/`[config]` legend).

A separate non-secret `deploy.env` (`REGISTRY` + SHA-pinned image tags) is GHA-written on every deploy — the two-file split keeps a compromised GitHub account from leaking runtime credentials while still letting CI control which image runs ([infra ADR-0007][in-0007]).

### Deploy pipeline (GHA → SSH → compose pull)

```
push to main
    │  GitHub Actions (.github/workflows/ci.yml)
    ▼   backend (fmt·clippy·test) · frontend (lint·check·test·build·e2e)
        compose (topology·replica·deploy·health-push self-tests) · zero-trust self-test
    │  (all four jobs must pass)
    ▼  deploy job: build images on GHA runners → push to GHCR
        ghcr.io/<owner>/second-brain-backend:sha-<SHA>
        ghcr.io/<owner>/second-brain-edge:sha-<SHA>   (CADDYFILE=Caddyfile.prod)
    │  SSH (command-restricted deploy key, forced command, no pty)
    ▼  pipe deploy.env (REGISTRY + 2 SHA tags, NO secrets) over stdin
    ▼  deploy.sh on VPS:
        1. validate against the ADR-0007 whitelist
        2. write /opt/second-brain/deploy.env
        3. fetch docker-compose.yml + litestream.yml + health-push.sh from the public repo @ <SHA>  (ADR-0010, atomic: fetch-all-then-install)
        4. docker compose --env-file deploy.env pull && up -d
```

Images build **on GHA runners** (a Rust release build spikes >2 GB RAM and would OOM the live brain on the VPS — [infra ADR-0003][in-0003]). The VPS never compiles; it only `docker compose pull`. The deploy key is locked down in `authorized_keys` with a forced `command="..."` — the command restriction *is* the security model: a leaked key can only restart existing containers. **Rollback** is a 30-second op: pipe a previous known-good SHA to `deploy.sh` over SSH (the previous image is cached; `pull` is a no-op; `up -d` reverts; config sync reverts the compose/litestream/health-push config to that SHA too).

### VPS bootstrap, Health Push, Disaster Recovery

- **`infrastructure/bootstrap.sh`** — idempotent provisioning for a fresh Debian VPS: 4 GB swap, Docker, an **INPUT-only** nftables firewall (Docker owns `FORWARD`), a `deploy` user, the install dir with an ADR-0010 ownership split (sync-eligible files deploy-owned; `deploy.sh` and the install dir root-owned — the gate the deploy key cannot replace), the Health Push cron, and the command-restricted deploy SSH key.
- **Health Push** ([infra ADR-0005][in-0005]) — a zero-RAM host-cron (every 5 min) that pushes to an ntfy.sh webhook when the Brain Replica stops replicating (metrics endpoint unreachable, or rising sync errors) or the Brain File volume nears capacity. The push model exists because silent replication drift is the one failure a pull-based admin tab cannot catch.
- **`infrastructure/DISASTER_RECOVERY.md`** — the human runbook for catastrophic VPS loss: `bootstrap.sh` → place `.env` → `litestream restore` from R2 into a fresh `sqlite_data` volume → `pull && up -d`. An untested restore is a hope, not a strategy — the procedure must be re-exercised on a throwaway VPS periodically.

### Real-world resource footprint

Measured on the live VPS (2 vCPU, 3.8 GiB RAM) with a 1-user browsing mix hitting the real read endpoints (`/graph`, `/thematic`, `/ontology`, `/merge-suggestions`) over ~3 min, sampled via `docker stats` @ 1s. No files changed, no WebAuthn scripted — a live `session_id` was borrowed read-only from the `sessions` table.

| Service | Idle Mem (cgroup) | Idle CPU | Under load | Per-request |
|---|---|---|---|---|
| **Backend** (Rust) | ~3.4 MiB | ~0% | +0.3 MiB, peak 0.13% | 0.6–1.3 ms |
| **Edge** (Caddy) | ~16.8 MiB | ~0% | unchanged (bypassed in test) | — |
| **Litestream** | ~31.3 MiB | ~0.08% | +0.3 MiB, peak 1.4% | — |
| **Host** | 650 MiB used, 3.27 GiB available | — | delta within noise | — |

**Headline:** at current scale (1 braindump, 5 concepts, 4 edges) the three services cost **~51 MiB of container RAM and ~0% CPU** — i.e. essentially their idle footprint. The expensive work is the **Gemini LLM calls** (`/braindumps` extraction, `/retrieve`, `/chat`), and those run on Google's servers — they cost latency + API spend + egress, *not* local RAM/CPU. As the brain grows, `/graph` (full gzipped snapshot) and `/thematic` (Louvain over `petgraph`) become the local cost centers. The `docker stats` (cgroup) vs `VmRSS` gap is real: most of the Rust binary's RSS is shared, evictable code pages; its unique heap is ~3–4 MiB. **Theoretical parallel capacity:** the 2 vCPU / 3.8 GiB box can sustain ~2000 local read requests per second (CPU-bound at ~1 ms each, with ~3 GiB free RAM allowing far more idle connections) — enough for thousands of concurrently-browsing users on reads — but the single-user Brain File architecture and the offloaded Gemini rate limits, not the VPS, are the real ceiling, so the box is nowhere near saturated.

---

## Repository layout

```
second_brain/
├── AGENTS.md                      # top-level agent-skills summary
├── CONTEXT-MAP.md                 # entry point for the multi-context layout
├── docker-compose.yml             # Edge + Backend + Litestream topology
├── docs/
│   ├── agents/                    # issue-tracker, triage-labels, domain docs
│   └── first_draft.md             # project vision / architecture (German)
├── backend/                       # Rust/Axum orchestrator & graph engine
│   ├── CONTEXT.md                 # backend controlled vocabulary
│   ├── Cargo.toml · Dockerfile
│   ├── src/                       # main, lib, routes/, auth/, graph_repo, db, llm, gemini, …
│   ├── tests/                     # 16 integration-test crates (one per issue slice)
│   └── docs/adr/                  # 10 backend ADRs
├── frontend/                      # SvelteKit (Svelte 5) PWA
│   ├── CONTEXT.md · AGENTS.md     # frontend vocabulary + commands
│   ├── package.json · svelte.config.js · vite.config.ts · playwright.config.ts
│   ├── static/                    # manifest.webmanifest, icons
│   ├── src/                       # routes/, lib/ (api, graph, capture, chat, state, …), service-worker.ts
│   ├── tests/                     # unit (Vitest) + e2e (Playwright)
│   └── docs/adr/                  # 5 frontend ADRs
└── infrastructure/                # Docker Compose, Caddy, deploy, DR
    ├── CONTEXT.md
    ├── edge/                      # Caddy Dockerfile + Caddyfile (+ Caddyfile.prod)
    ├── bootstrap.sh · deploy.sh · health-push.sh · health-push.cron
    ├── litestream.yml · .env.example · DISASTER_RECOVERY.md
    ├── keys/deploy.pub            # public half of the deploy key (no private key in repo)
    ├── test/                      # topology.sh · replica.sh · deploy.sh · zero-trust.sh
    └── docs/adr/                  # 10 infrastructure ADRs
```

---

## Local development

### Full stack (Docker Compose)

```bash
docker compose up        # Edge :80/:443, Backend internal, Litestream sidecar
docker compose down      # keeps the Brain File (named volume sqlite_data)
docker compose down -v   # destroys the Brain File
```

Without a `deploy.env`, image tags fall back to `:latest` and Compose builds the Edge + Backend images locally. The Edge serves the PWA at `http://localhost` and reverse-proxies `/api/*` to the backend. (Litestream will crash-loop without real R2 credentials in `infrastructure/.env`; comment it out for purely local dev.)

### Backend (Rust)

```bash
cd backend
cargo test --all        # unit + integration (FakeLlm, in-memory SQLite, no Gemini key needed)
cargo run               # serves on 0.0.0.0:8080; DATABASE_URL defaults to second_brain.db
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

With `GEMINI_API_KEY` unset, `GeminiClient::from_env()` returns `None` and the backend wires `FakeLlm` — so the full pipeline runs without network calls. `Config::for_tests()` uses `:memory:` SQLite.

### Frontend (SvelteKit)

```bash
cd frontend
npm ci
npm run dev             # Vite dev server
npm run build           # adapter-static PWA Bundle into build/
npm run preview         # serve the built bundle
npm test                # Vitest unit/component
npm run test:e2e        # Playwright (builds first, previews on 127.0.0.1:4173)
npm run check           # svelte-check typecheck
npm run lint            # ESLint
```

The frontend calls `/api` by default (the Edge reverse-proxy). For local dev against a backend on another origin, set `VITE_BACKEND_BASE_URL` (see `frontend/.env.example`). Deepgram STT needs `VITE_DEEPGRAM_API_KEY`; without it, capture falls back to the Web Speech API.

---

## Configuration

| Where | What |
|---|---|
| `infrastructure/.env.example` | The authoritative runtime-secret/config key list (committed, values blank). Copy to `infrastructure/.env` on the VPS and fill in. |
| `frontend/.env.example` | `VITE_BACKEND_BASE_URL` (defaults to `/api`). `VITE_DEEPGRAM_API_KEY` is read inline for STT. |
| `backend/src/config.rs` | Env-driven backend config: `HOST`, `PORT`, `DATABASE_URL`, `RUST_LOG`, `LOG_FORMAT`, `WEBAUTHN_RP_ID/NAME/ORIGIN`. |

Runtime secrets (never committed, GHA-blind): `GEMINI_API_KEY`, `LITESTREAM_ACCESS_KEY_ID` / `LITESTREAM_SECRET_ACCESS_KEY`, `NTFY_WEBHOOK_URL`. Non-secret config: `LITESTREAM_ENDPOINT`/`LITESTREAM_BUCKET`, Gemini model ids, WebAuthn RP, backend bind/port. **`WEBAUTHN_RP_ID` must match the hostname in `Caddyfile.prod`.**

---

## Testing

The project is sliced vertically — tests map 1:1 to ADRs and backend issues.

- **Backend** — `cargo test --all`. 16 integration-test crates under `backend/tests/` (health, auth, admin_logs, ontology, braindump, extraction, deletion, merge_suggestions, retrieval, ontology_governance, chat, chat_inferences, thematic, chat_macrostructure, topology_snapshot, delta_sync) drive the router via `tower::ServiceExt::oneshot` with in-memory SQLite and a scripted LLM stand-in. Plus extensive `#[cfg(test)]` unit tests inside each src module (migrations, FakeLlm characterization, deterministic vectors, Louvain, synthesis-prompt content, ingest without HTTP).
- **Frontend** — Vitest (jsdom + `fake-indexeddb` + `@testing-library/svelte`) for ~46 unit/component test files; Playwright (chromium) e2e for PWA shell/installability, auth + cookie-reload, graph rendering + Frozen Graph fallback + 2D-on-iOS, and the admin log viewer.
- **Infrastructure** — four dependency-free bash test scripts under `infrastructure/test/`: `topology.sh`, `replica.sh` (with `--live` MinIO round-trip), `deploy.sh`, `zero-trust.sh` (self-testing). All run in the GHA `compose` and `zero-trust` jobs.

CI (`.github/workflows/ci.yml`) runs all of the above on every push to `main` and every PR; the `deploy` job runs only on push to `main` after all four test jobs pass.

---

## Architecture decisions (ADR index)

25 ADRs total, scoped per context (no root-level `docs/adr/`). All are in effect; "refined by" denotes evolution, not supersession.

### Backend (`backend/docs/adr/`)

| # | Title |
|---|---|
| 0001 | Concept identity via embedding match, with borderline-confirm hybrid |
| 0002 | Edges are typed, directional, and accrete provenance |
| 0003 | Ontology governance: fractal tolerance split, event-sourced refactor, pinned model |
| 0004 | Retrieval is seed-then-expand: the graph is load-bearing, vectors are seed and backfill |
| 0005 | Chat is grounded synthesis: mandatory citations, graph-constrained inference, silence when unsupported |
| 0006 | Chat writes back: governed inference proposals, origin-typed by epistemic class |
| 0007 | Ingest fires on explicit submit; braindumps are immutable thought-snapshots, edits are error-correction only |
| 0008 | Frontend is strictly a view; thematic clustering is a backend-owned read model |
| 0009 | Thematic-origin proposals carry a frozen evidence snapshot; endorsement is immutable |
| 0010 | Concept provenance is extraction-based, symmetric to edge provenance |

### Frontend (`frontend/docs/adr/`)

| # | Title |
|---|---|
| 0001 | The Consumer Rule: the computational boundary is defined by who consumes the output |
| 0002 | Whole-graph fetch on load, pull-on-focus for deltas, no real-time push |
| 0003 | The user edits the verbatim, never the cleaned rendering |
| 0004 | The HITL queue is bifurcated by epistemic weight, not by the backend's fractal shape |
| 0005 | One named write-intent exception to "strictly a view"; the Service Worker stays dumb |

### Infrastructure (`infrastructure/docs/adr/`)

| # | Title |
|---|---|
| 0001 | Two-service Compose topology with PWA baked into Caddy, rejecting PaaS |
| 0002 | Continuous WAL replication of the Brain File to offsite object storage |
| 0003 | Deploy pipeline: build on GHA, push to GHCR, VPS pulls via command-restricted SSH key |
| 0004 | Secret model: .env on host, GHA-blind, Zero-Trust Images |
| 0005 | Observability: pull-based admin tab for debugging, push-based cron for survival |
| 0006 | Backend internal-only: Caddy is the sole published port |
| 0007 | SHA-pinned image tags with a separate Deploy State file for instant rollback |
| 0008 | Disaster recovery: runbook as human source of truth, bootstrap script as automation, tested periodically |
| 0009 | Zero-Trust Image contract: .env.example source-of-truth + CI guard |
| 0010 | Sync infra config from the public repo at the deployed SHA on every deploy |

---

## Agent skills & conventions

This repo is set up for the Matt Pocock engineering skills. [`AGENTS.md`](./AGENTS.md) summarizes the conventions; [`docs/agents/`](./docs/agents) holds the detail:

- **Issue tracker** — GitHub issues via the `gh` CLI; external PRs are *not* a triage surface. Auto-close an issue with `Closes #<n>` in the commit message or PR description.
- **Triage labels** — five canonical roles map 1:1 to GitHub labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`.
- **Domain docs** — multi-context: `CONTEXT-MAP.md` → per-context `CONTEXT.md` (frontend, backend, infrastructure), each with its own `docs/adr/`. Use the glossaries' exact vocabulary; flag — don't silently override — any output that contradicts an ADR.

---

## Notes

- The project is **single-user, German-first** (`docs/first_draft.md` is the vision doc, written in German). Deepgram STT defaults to `de`; Web Speech fallback uses `de-DE`.
- **No LLM is self-hosted.** Extraction, cleaning, synthesis, and embeddings all go through hosted Gemini APIs; the VPS stays slim and the pipeline stays deterministic. The custom Rust graph engine is intentionally stricter than LightRAG (typed edges, origin-typed provenance, governed ontology, event-sourced type history) — the project borrows LightRAG's retrieval *algorithm*, not its library.
- The repo is **public**, which is what makes ADR-0010's config-sync-on-deploy safe (fetch from `raw.githubusercontent.com` needs no auth and introduces no new secret).

[be-0001]: ./backend/docs/adr/0001-concept-identity-via-embedding-match.md
[be-0002]: ./backend/docs/adr/0002-edges-typed-directional-accreting.md
[be-0003]: ./backend/docs/adr/0003-ontology-governance-fractal-tolerance-event-sourced-refactor.md
[be-0004]: ./backend/docs/adr/0004-retrieval-seed-then-expand-graph-load-bearing.md
[be-0005]: ./backend/docs/adr/0005-chat-grounded-synthesis-citations-graph-constrained-inference.md
[be-0006]: ./backend/docs/adr/0006-chat-writes-back-governed-inference-proposals-origin-typed-provenance.md
[be-0007]: ./backend/docs/adr/0007-ingest-on-submit-immutable-thought-snapshots-error-correction-edits.md
[be-0008]: ./backend/docs/adr/0008-frontend-strictly-view-thematic-read-model-backend-owned.md
[be-0009]: ./backend/docs/adr/0009-thematic-inference-frozen-evidence-snapshot-endorsement-immutable.md
[be-0010]: ./backend/docs/adr/0010-concept-extraction-provenance-symmetric-to-edges.md
[fe-0001]: ./frontend/docs/adr/0001-the-consumer-rule.md
[fe-0002]: ./frontend/docs/adr/0002-whole-graph-fetch-pull-on-focus-no-push.md
[fe-0003]: ./frontend/docs/adr/0003-edit-verbatim-never-cleaned.md
[fe-0004]: ./frontend/docs/adr/0004-hitl-queue-bifurcated-by-epistemic-weight.md
[fe-0005]: ./frontend/docs/adr/0005-one-named-write-intent-exception-service-worker-stays-dumb.md
[in-0001]: ./infrastructure/docs/adr/0001-two-service-topology-pwa-baked-into-caddy-reject-paas.md
[in-0002]: ./infrastructure/docs/adr/0002-continuous-wal-replication-of-brain-file-to-r2.md
[in-0003]: ./infrastructure/docs/adr/0003-deploy-pipeline-build-on-gha-push-to-ghcr-command-restricted-ssh-pull.md
[in-0004]: ./infrastructure/docs/adr/0004-secret-model-env-on-host-gha-blind-zero-trust-images.md
[in-0005]: ./infrastructure/docs/adr/0005-observability-pull-admin-tab-push-cron-for-survival.md
[in-0006]: ./infrastructure/docs/adr/0006-backend-internal-only-caddy-sole-published-port.md
[in-0007]: ./infrastructure/docs/adr/0007-sha-pinned-tags-deploy-state-file-for-instant-rollback.md
[in-0008]: ./infrastructure/docs/adr/0008-disaster-recovery-runbook-and-bootstrap-script-tandem.md
[in-0009]: ./infrastructure/docs/adr/0009-zero-trust-image-contract-env-example-and-ci-guard.md
[in-0010]: ./infrastructure/docs/adr/0010-sync-infra-config-from-public-repo-at-deployed-sha.md
