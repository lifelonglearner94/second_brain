# Context Map

## Contexts

- [Backend](./backend/CONTEXT.md) — the Rust orchestrator and graph engine; owns the core data model (braindumps, concepts, the knowledge graph)
- [Frontend](./frontend/CONTEXT.md) — the PWA UI: voice capture, 3D graph visualization, chat _(planned; not yet created)_
- [Infrastructure](./infrastructure/CONTEXT.md) — Docker Compose, reverse proxy, CI/CD, hosting _(planned; not yet created)_

## Relationships

- **Frontend → Backend**: the PWA calls the backend over HTTP — to submit braindumps, read the graph, and run chat/retrieval. The backend owns the data model; the frontend is a view over it.
