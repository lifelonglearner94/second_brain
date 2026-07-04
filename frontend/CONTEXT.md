# Frontend

The PWA UI for Second Brain — voice capture of braindumps, 3D graph visualization of the knowledge graph, and the chat surface. A strict view over the backend: it renders the backend's data model and never computes anything whose output is consumed by anything other than the display (ADR-0001).

## Language

**Spatial View-Graph**:
The ephemeral, locally mutated representation of the backend's data model (concepts, typed edges, Louvain cluster IDs), augmented strictly with the geometric and visual properties (x/y/z, velocity, visibility) required for WebGL rendering. The semantic content is fetched from the backend and cached; the spatial state is generated locally by ForceAtlas2 and dies with the session. Caching pre-computed coordinates would make the backend a layout engine, violating the backend's ADR-0008. Populated on bootstrap by the Global Topology Snapshot and reconciled on focus by the Delta Sync.
_Avoid_: render cache, view model, graph snapshot, scene, layout

**Global Topology Snapshot**:
The unit of bootstrap fetch — a single compressed JSON payload from the backend containing all nodes, typed edges, and the current Louvain partition IDs. Loaded wholesale into the Spatial View-Graph on app start, after which ForceAtlas2 runs locally to generate spatial state. Licensed by Personal-Scale: a prolific user (≈10 concepts/day) takes a decade to reach 35k concepts, well under the 50k-node ceiling at which WebGL + ForceAtlas2 hold 60fps on mobile.
_Avoid_: graph dump, full graph response, initial load

**Delta Sync**:
The unit of incremental fetch — a payload of changes since last_sync_timestamp, pulled on window focus (and overlaid after a braindump ingestion response) to reconcile the Spatial View-Graph with backend truth. Carries additions (new concepts/edges from ingests the user didn't trigger, and retags from async ontology refactors — backend ADR-0003) as well as deletions. Replaces real-time push: the backend stays a stateless Axum server because the system is single-user — there is no collaborating actor to sync against, so brief visual staleness between focus events is an acceptable cost for not running WebSocket/SSE infrastructure. (See ADR-0002.)
_Avoid_: refresh, sync, poll, update

**Active Capture**:
The ephemeral, mutable frontend text buffer that accumulates streaming STT output (Deepgram Nova-3 online, Web Speech API offline) and manual keystrokes, representing a thought in the process of being formed before it is committed to the system. Not a braindump — nothing in this state is immutable, tracked, or seen by the backend; backspacing, rewording, and fixing STT hallucinations here are temporary keystrokes, not graph edits. The text becomes a braindump only at explicit submit (backend ADR-0007), at which point the Active Capture's contents become the verbatim source of truth and the buffer is discarded.
_Avoid_: draft, transcript, recording, utterance, input

**Pending Captures**:
Offline Active Captures persisted to a local IndexedDB queue when the user submits without connectivity — or when Deepgram is unreachable and the fallback STT (Web Speech API / keyboard) filled the buffer. Distinct from Active Capture: these have crossed the user's intent-to-submit but not the network boundary, so they await explicit review-and-confirm on reconnect. Never auto-submitted: because offline STT is significantly less accurate, the UI presents a "Pending Captures" list on reconnect and the user must correct, confirm, and hit submit for each before it becomes a braindump. Strict review-and-confirm is the offline extension of ADR-0007's explicit-submit rule.
_Avoid_: outbox, offline queue, pending submits, draft queue

**Housekeeping Queue**:
The frontend HITL surface for semantic housekeeping — concept-merge and ontology-type-merge confirmations (backend ADR-0001/0003), where the system found two things that are probably the same and asks the user to confirm. Low epistemic weight: the user is arbitrating similarity, not trust. The action verb is "Merge." The backend exposes these through the same fractal "Merge Suggestion" API as chat inferences; the frontend deliberately bifurcates them into this separate queue so the user isn't context-switching between housekeeping and epistemic leaps. (See ADR-0004.)
_Avoid_: clean up tab, merge queue, confirmation queue, duplicate queue

**Endorsement Queue**:
The frontend HITL surface for epistemic endorsements — chat-inferred edges (backend ADR-0006, structural and thematic), where the LLM proposes a new connection and the user must approve it before it persists. Higher epistemic weight than the Housekeeping Queue: the user is arbitrating trust in an LLM deduction, not similarity. The action verb is "Approve Connection," distinct from the Housekeeping Queue's "Merge." Endorsed edges are optimistically merged into the Spatial View-Graph immediately (same action-driven local-merge pattern as braindump ingestion — ADR-0002). (See ADR-0004.)
_Avoid_: new insights tab, inference queue, approval queue, suggestion queue

**Evidence Disclosure**:
The frontend's pattern for surfacing the basis of a proposed edge in the Endorsement Queue — not via academic labels ("Structural"/"Thematic") but via the evidence payload itself. Structural proposals (backend ADR-0006, graph-backed) expand to show the traversable node-edge-node chain under "Based on existing path." Thematic proposals (backend ADR-0006, statistical hypothesis) expand to show the Thematic Snapshot (backend ADR-0009) — the frozen cluster of braindumps that motivated the proposal — under "Based on thematic density." The snapshot is visible at proposal time so the user isn't blindly guessing, and remains accessible for retroactive audit. The distinction maps to the backend's two origin types but is presented to the user as a difference in *what they're looking at*, not *what it's called*.
_Avoid_: evidence panel, proof expander, inference details, justification

**Document Modal**:
The isolated braindump reader opened by clicking a citation chip in chat. Renders a single braindump's text (cleaned by default, verbatim behind toggle — ADR-0003) without moving the Spatial View-Graph camera. Exists because jumping the camera while reading a chat answer disrupts cognitive flow — citations are a reading interaction, not a navigation interaction. The citation chip is inline in the chat text (e.g. `[1]`), and the traversal path that produced the citation (seed concept → traversed edges → collected braindumps, backend ADR-0004) is hidden from the user; only the final cited braindumps surface, not the graph mechanics that retrieved them.
_Avoid_: braindump viewer, citation popover, source reader, reading pane

**Explicit Silence**:
The UI state rendered when chat cannot find graph-supported evidence to answer a query (backend ADR-0005: "when the graph doesn't support an answer, chat is silent"). Distinct from a blank response, a loading state, or a network error: the UI displays an explicit textual acknowledgment — "I cannot find graph-supported evidence to answer this" — signaling that the system considered the question and has no grounded answer. A blank UI would read as a crash or timeout; explicit silence reads as integrity.
_Avoid_: empty state, no results, blank response, failed query

**Frozen Graph**:
The offline read-only rendering mode: when the PWA opens without connectivity, the frontend loads the last Global Topology Snapshot from its IndexedDB cache into the Spatial View-Graph and renders it with a visible staleness indicator ("Showing graph as of [last sync timestamp] — offline"). Chat is unavailable (needs the backend LLM), extractions don't run (needs the backend pipeline), and new braindump submissions route to Pending Captures. Remains a view, not a replica: the offline client cannot compute new semantics — it is a frozen topographical map of the last known state, not a co-owner of the model. (See ADR-0005.)
_Avoid_: offline mode, stale view, cached graph, offline graph

**Viewport State**:
The camera position, zoom level, and currently selected node ID — the minimal navigational context that lets the PWA feel native rather than giving the user amnesia on every tab switch. Saved to LocalStorage (synchronous, tiny, ideal for simple key-value UI state) and restored on reload: the layout physics regenerates from scratch, but the camera snaps back to the saved coordinates and the active node is re-highlighted. Distinct from the Spatial View-Graph's spatial state (x/y/z node positions, velocities — ephemeral, regenerated by ForceAtlas2 each session) — Viewport State is the *observer's* state, not the *graph's* state. Not backend data: the backend has no concept of where the camera is, so this is pure frontend-owned UI state and does not violate ADR-0001.
_Avoid_: camera state, view position, session state, navigation state
