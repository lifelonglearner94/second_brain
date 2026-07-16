# 0001 — Braindump → Graph visibility delay (and the "first braindump appears only after the second" symptom)

Research issue: #92. Scope: trace the exact path from braindump submit to Spatial View-Graph visibility, pin down the root cause of the reported symptom ("first braindump only appears after a second one is entered"), and evaluate speedup options that preserve extraction quality, ADR-0007 immutability, and per-user isolation.

## TL;DR

The visibility delay is **not pure backend latency** — it is a **frontend sync-timing bug**. The backend submit is fire-and-forget: the verbatim is persisted in milliseconds and the `clean → extract → accrete` pipeline runs in a background `IngestRunner`. Concepts/edges enter the graph only when that background pipeline commits (seconds later, after two Gemini calls). The frontend, however, fires a single `GET /graph/delta` **immediately** after the submit response returns — which races the background pipeline and returns an **empty** delta — and then has **no trigger to re-sync** until the window regains focus. The second braindump's immediate delta call is what finally surfaces the first one. A secondary correctness hole: the cursor is advanced to "now" on that empty delta, so a same-second background commit can be permanently skipped by the strict-`>` delta filter.

Recommended fix: a pull-based **ingest-status poll** after submit (the `ingest_status` column already exists) plus a **concurrent `clean` + `extract`** backend optimization. Both preserve quality, ADR-0007, and per-user isolation. See the follow-up build issue linked at the bottom.

---

## 1. The exact braindump → graph visibility path

### 1.1 Backend submit: persist verbatim, return immediately, spawn background ingest

`POST /braindumps` → `routes::braindump::submit` (`backend/src/routes/braindump.rs:52`):

1. Validate non-empty verbatim (`braindump.rs:58`).
2. `braindump::submit_braindump` (`backend/src/braindump.rs:155`) → `insert_braindump` with `cleaned = ""` as a placeholder (`braindump.rs:156`). The row is `ingest_status = 'pending'` by column default (`backend/src/db.rs:257`). **No LLM call is on the request path** — the HTTP response returns the braindump row in milliseconds.
3. `state.ingest_runner.spawn(...)` (`routes/braindump.rs:66`) fires-and-forgets the background loop via `tokio::spawn` (`braindump.rs:345`) and returns the row (`routes/braindump.rs:76`).

### 1.2 Background pipeline: clean → extract → accrete (the only place concepts/edges are committed)

`IngestRunner::spawn` → `run_ingest_loop` (`braindump.rs:203`) → `process_ingest_once` (`braindump.rs:167`):

1. Load braindump, idempotency guard on `ingest_status == "complete"` (`braindump.rs:178`).
2. `llm.clean(&braindump.verbatim)` (`braindump.rs:183`) — **Gemini call #1**, takes seconds. Result overwrites `cleaned` in place via `overwrite_verbatim` (`braindump.rs:184`); `verbatim` + `created_at` untouched (ADR-0007).
3. `accrete` (`braindump.rs:127`):
   - `graph::ontology_slugs` (`braindump.rs:133`).
   - `llm.extract(&braindump.verbatim, &ontology)` (`braindump.rs:134`) — **Gemini call #2**, takes seconds. **Runs on the verbatim, not the cleaned rendering** (confirmed: the `Llm::extract` signature is `extract(&self, verbatim: &str, ...)` at `backend/src/llm.rs:47`; `accrete` passes `&braindump.verbatim`).
   - `graph::ingest_extraction(...)` (`braindump.rs:135`) — the **atomic accretion** that commits concepts + edges to the graph with `created_at = now_seconds()`. This is the instant the graph actually changes.
4. On success, `run_ingest_loop` marks `ingest_status = "complete"` (`braindump.rs:225`). On a transient failure it stays `pending` and retries after `config.ingest_retry_interval_secs` (`braindump.rs:261`); on a non-retryable failure it is terminal'd `failed` (`braindump.rs:269`).

**So the graph does not change at submit time. It changes only when the background pipeline commits, which is at least `clean + extract` Gemini latency (two sequential network calls) after submit.**

### 1.3 Frontend: single racing delta call, then wait-for-focus

`ActiveCaptureStore.submit` (`frontend/src/lib/capture/active-capture.svelte.ts:74`) → `ingest.ingest(verbatim)` → `createIngestApi.ingest` (`frontend/src/lib/capture/ingest.ts:29`):

```ts
const braindump = await client.submitBraindump(verbatim);   // returns in ms
const delta = await client.getGraphDelta(getCursor());      // fires IMMEDIATELY
return { braindump, concepts: delta.added_concepts, edges: delta.added_edges, cursor: delta.cursor };
```

`getCursor()` reads `graphStore.cursor` (`frontend/src/routes/app/+page.svelte:18`). This delta call runs **before the background pipeline has committed**, so for the just-submitted braindump `delta.added_concepts` / `delta.added_edges` are **empty**. The response's `cursor` is `now_seconds()` at this query time (`backend/src/delta.rs:85`, `backend/src/routes/delta.rs:33`).

`onIngest(res)` → `graphStore.mergeIngest(res)` (`frontend/src/lib/state/graph.svelte.ts:77`):

- Applies the (empty) delta to the snapshot (`graph.svelte.ts:80`).
- **Unconditionally sets `this.cursor = res.cursor`** (`graph.svelte.ts:90`) — the cursor advances to "now" even though nothing was added.
- Rebuilds `this.data` (`graph.svelte.ts:91`).

The only **other** sync trigger is on the graph page: `onWindowFocus(globalThis, reconcileOnFocus)` (`frontend/src/routes/app/graph/+page.svelte:126`), which calls `graphStore.syncDelta(apiClient)` (`graph/+page.svelte:140`). There is **no polling**, **no trigger when the background ingest completes**, and **navigating to the graph page does not re-sync** — `loadFromNetworkOrCache` short-circuits and returns the cached snapshot when `this.snapshot` is already set (`graph.svelte.ts:42`), and route navigation does not fire a `window` `focus` event.

Finally, the 3D/2D graph is an imperative renderer; updating `graphStore.data` does **not** re-render it. The graph is re-rendered only inside `reconcileOnFocus` when `hasDeltaChanges(delta)` is true (`graph/+page.svelte:142`).

### 1.4 The delta filter that makes the cursor matter

`graph_delta` filters additions by strict `created_at > since` (`backend/src/delta.rs:16-21`); the returned cursor is `now_seconds()` at query time. So any concept whose `created_at` is `<=` the client's cursor is **never** returned by a future delta — only a full `GET /graph` snapshot reload (`routes/snapshot.rs`) would recover it.

---

## 2. Root cause of "first braindump appears only after the second"

Two compounding bugs; **Bug A is the dominant, always-present cause**, Bug B is a same-second edge that makes it worse.

### Bug A — racing delta + no post-completion sync trigger (primary)

Timeline for a user who submits braindump **A** and stays on the page (no window focus change):

1. **T0** — submit A. `submitBraindump` returns in ms; background ingest A is spawned.
2. **≈T0** — frontend fires `getGraphDelta(cursor=C0)`. Background pipeline A has **not** committed (it is mid-`clean`/`extract` Gemini call). Delta is **empty** for A. `mergeIngest` advances `graphStore.cursor` to `C1 ≈ now_seconds() ≈ T0`.
3. **T1 = T0 + (clean + extract + accrete) ≈ T0 + a few seconds** — background pipeline A commits; A's concepts/edges land with `created_at = T1`. **Nothing tells the frontend to re-sync.** The user is still on the page; no `focus` event fires; navigating to `/app/graph` returns the cached (stale) snapshot.
4. **T2 (later)** — user submits braindump **B**. Frontend fires `getGraphDelta(cursor=C1)`. Now A's concepts have `created_at = T1 > C1`, so **A's concepts/edges are returned** (B's are still pending). `mergeIngest` merges A into the store.

→ **The second braindump's immediate delta call is what surfaces the first braindump.** This is exactly the reported symptom: "the first braindump only appeared after they entered a second one." It is a frontend sync-timing bug (race + missing re-sync trigger + cursor advanced on an empty delta), **not** pure backend latency. Backend latency only sets the size of the window; the bug is that the frontend never looks again until a focus event or the next submit.

### Bug B — cursor advance on empty delta + strict-`>` filter (secondary, same-second lost-update)

`mergeIngest` advances the cursor to `res.cursor` even when the delta is empty (`graph.svelte.ts:90`). Combined with strict `created_at > since` filtering (`delta.rs:16-21`), if the background pipeline commits in the **same second** as the immediate post-submit `getGraphDelta` (fast/cached Gemini, or the test `IngestRunner::new_inline`), then `created_at == cursor` and **strict `>` excludes it forever** — every future delta with that cursor skips A's concepts. Recoverable only by a full `GET /graph` snapshot reload. In production Gemini latency usually puts the commit in a later second, so Bug A dominates; but Bug B is a real correctness hole and is what makes the inline/test path and any future fast-LLM scenario flaky.

---

## 3. Speedup options evaluated against "without losing quality"

Quality constraints: (a) extraction accuracy, (b) ADR-0007 immutability (verbatim is the source of truth; `cleaned` is a derived rendering overwritten in place; edits are error-correction only), (c) per-user isolation (accretion is scoped by `user_id`, `braindump.rs:133-142`, `routes/braindump.rs:72`), and (d) the delta surface's stated design: **stateless, pull-only, no WebSocket/SSE, no server-held session** (`delta.rs:5-8`).

### Option 1 — Backend: run `clean` and `extract` concurrently ✅ RECOMMENDED (quality-neutral)

`accrete` calls `llm.extract(&braindump.verbatim, ...)` (`braindump.rs:134`) — on the **verbatim**, not the cleaned rendering. `process_ingest_once` calls `llm.clean(&braindump.verbatim)` (`braindump.rs:183`) then `accrete` (`braindump.rs:187`). The two LLM calls are **independent** (extract does not consume `cleaned`); today they run strictly sequentially.

- **Change:** in `process_ingest_once`, `tokio::join!` the `clean` future and a future that loads the ontology + runs `extract` on the verbatim, then `overwrite_verbatim` with the cleaned result, then `ingest_extraction`. Critical path shrinks from `clean + extract + accrete` to `max(clean, extract) + accrete`.
- **Quality:** **none lost.** Identical inputs to both LLM calls; identical accretion; `cleaned` is still derived and overwritten in place; extract still runs on the verbatim. No ADR-0007 change.
- **Immutability:** preserved — `overwrite_verbatim` still keeps `id` + `created_at` (`braindump.rs:68`).
- **Per-user isolation:** preserved — both calls are scoped to the same `user_id`/braindump.
- **Caveat:** this **reduces the background pipeline window** (so the poll in Option 2 runs shorter) but does **not by itself fix the visibility symptom** — the frontend still races the pipeline and still has no re-sync trigger. It is a complement to Option 2, not a substitute.

### Option 2 — Frontend: pull-based ingest-status poll until complete, then delta-sync ✅ RECOMMENDED (the actual fix)

The `ingest_status` column already exists (`db.rs:257`) and is toggled `pending → complete | failed` by `run_ingest_loop` (`braindump.rs:225`, `braindump.rs:269`); `db::get_ingest_state` already reads it (`db.rs:737`). There is **no** HTTP route exposing it today (the protected routes in `routes/mod.rs:63-101` have no ingest-status endpoint).

- **Change (backend):** add `GET /braindumps/{id}/ingest-status` returning `{ status, attempts }` via `db::get_ingest_state`. Pure read; registered under the protected layer like the other braindump routes.
- **Change (frontend):** in `createIngestApi.ingest` (`ingest.ts:29`), after `submitBraindump` returns, **do not** treat the single racing `getGraphDelta` as final. Instead poll: with backoff (e.g. 400ms → 800ms → 1.6s …), call `getIngestStatus(braindump.id)`; when it returns `complete`, do one final `getGraphDelta` and merge+advance cursor; when `failed`, stop (the admin logs surface it); on a timeout (e.g. ~12s), stop **without advancing the cursor** so the next focus/submit sync still catches it. Equivalently, poll `getGraphDelta` directly until it non-empty or timeout — but the status endpoint is cheaper and gives a deterministic "done" signal rather than guessing from delta contents.
- **Fix Bug B too:** only advance `graphStore.cursor` when a delta was actually applied with changes (or on the final post-`complete` sync), not on every empty racing delta (`graph.svelte.ts:90`).
- **Quality:** **none lost.** This is read-only client behavior; extraction, accretion, immutability, and isolation are untouched.
- **Architecture:** **respects the stateless pull-only design** (`delta.rs:5-8`) — it is just an additional pull, not a push channel. No WebSocket/SSE, no server-held session.
- **This is the fix that actually resolves the reported symptom** — the graph updates as soon as the background ingest completes, without waiting for a window focus or a second submit.

### Option 3 — Frontend: delta-sync on graph-page mount/navigation (cheap supplement)

Currently `loadFromNetworkOrCache` returns the cached snapshot and does not re-sync on navigation (`graph.svelte.ts:42`); the graph only reconciles on `window` `focus` (`graph/+page.svelte:126`).

- **Change:** when the graph page mounts and `this.snapshot` is already loaded, also call `graphStore.syncDelta(apiClient)` (online-only). Low cost; no quality impact.
- **Limits:** helps the "submit on home → navigate to graph" flow, but does **not** help a user who stays on the graph page (no navigation, no focus). A supplement to Option 2, not a replacement.

### Option 4 — Backend: server push (SSE/WebSocket) on ingest complete ❌ REJECTED

Would give instant visibility but violates the explicit delta-surface design constraint ("no WebSocket, no SSE, no server-held session", `delta.rs:5-8`) and adds server-held state for a single-user app where brief staleness was deemed acceptable. Option 2's pull-based poll reaches the same outcome within the architecture.

---

## 4. Recommendation

Ship **Option 2 (frontend ingest-status poll + stop advancing the cursor on empty deltas)** as the primary fix — it directly resolves the "first braindump appears only after the second" symptom and closes the Bug B same-second lost-update hole, with zero quality/ADR-0007/isolation impact and within the stateless pull-only architecture. Ship **Option 1 (concurrent `clean` + `extract`)** alongside it as a quality-neutral backend speedup that shortens the window the poll runs. Optionally add **Option 3** as a small UX supplement.

Follow-up build issue: see "Issue link" below (created via `gh issue create`, labeled `enhancement` + `ready-for-agent`).

---

## 5. Key file references

| Concern | Location |
| --- | --- |
| Submit route (fire-and-forget) | `backend/src/routes/braindump.rs:52` (`submit`), `:61` (`submit_braindump`), `:66` (`ingest_runner.spawn`) |
| Persist verbatim + empty cleaned | `backend/src/braindump.rs:155` (`submit_braindump`) |
| Background retry loop | `backend/src/braindump.rs:203` (`run_ingest_loop`) |
| Single ingest attempt (clean → accrete) | `backend/src/braindump.rs:167` (`process_ingest_once`) |
| `accrete` (extract on **verbatim** + atomic commit) | `backend/src/braindump.rs:127`, `:134` (`llm.extract(&braindump.verbatim, …)`), `:135` (`ingest_extraction`) |
| `Llm::extract` signature (verbatim, not cleaned) | `backend/src/llm.rs:47` |
| `ingest_status` column + state read | `backend/src/db.rs:257`, `:737` (`get_ingest_state`), `:765` (`set_ingest_status`) |
| IngestRunner (spawn / inline) | `backend/src/braindump.rs:300`, `:334` (`spawn`) |
| Delta route + filter (strict `>`, cursor = now) | `backend/src/routes/delta.rs:27`, `backend/src/delta.rs:16-21`, `:85` |
| Frontend ingest: racing `getGraphDelta` | `frontend/src/lib/capture/ingest.ts:29` |
| `mergeIngest`: cursor advanced on empty delta | `frontend/src/lib/state/graph.svelte.ts:77`, `:90` |
| Graph page: only `onWindowFocus` reconciles | `frontend/src/routes/app/graph/+page.svelte:126`, `:136` (`reconcileOnFocus`) |
| `loadFromNetworkOrCache` cache short-circuit (no re-sync on nav) | `frontend/src/lib/state/graph.svelte.ts:42` |
| API client submit/delta | `frontend/src/lib/api/client.ts:467` (`submitBraindump`), `:474` (`getGraphDelta`) |
| ADR-0007 (immutability, edits = error-correction) | `backend/docs/adr/0007-ingest-on-submit-immutable-thought-snapshots-error-correction-edits.md` |
| Delta-surface design (stateless, pull-only) | `backend/src/delta.rs:5-8` |
