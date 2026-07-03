# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Layout: multi-context

This repo is a monorepo with multiple contexts. Read `CONTEXT-MAP.md` at the root — it points at one `CONTEXT.md` per context. Read each one relevant to the topic.

Planned contexts (per `docs/first_draft.md`):
- `frontend/` — the PWA UI (TypeScript/React/Svelte, Web Speech API, 3D graph viz)
- `backend/` — the Rust/Axum orchestrator and LightRAG graph engine
- `infrastructure/` — Docker Compose, reverse proxy, CI/CD, hosting

## Before exploring, read these

- **`CONTEXT-MAP.md`** at the repo root — points at one `CONTEXT.md` per context. Read each one relevant to the topic.
- **`docs/adr/`** — system-wide decisions. Read ADRs that touch the area you're about to work in.
- **`<context>/docs/adr/`** — context-scoped decisions (e.g. `frontend/docs/adr/`, `backend/docs/adr/`).

If any of these files don't exist, **proceed silently**. Don't flag their absence; don't suggest creating them upfront. The `/domain-modeling` skill (reached via `/grill-with-docs` and `/improve-codebase-architecture`) creates them lazily when terms or decisions actually get resolved.

## File structure

```
/
├── CONTEXT-MAP.md
├── docs/adr/                          ← system-wide decisions
├── frontend/
│   ├── CONTEXT.md
│   └── docs/adr/                      ← frontend-specific decisions
├── backend/
│   ├── CONTEXT.md
│   └── docs/adr/                      ← backend-specific decisions
└── infrastructure/
    ├── CONTEXT.md
    └── docs/adr/
```

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in the relevant `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (event-sourced orders) — but worth reopening because…_
