# Ontology governance: fractal tolerance split, event-sourced refactor, pinned model

The ontology reuses the *exact* identity-resolution pattern from ADR-0001 (embedding-match + merge suggestions) - fractally, at the type layer - but with a stricter tolerance. **Tolerance is inversely proportional to blast radius:** a wrong concept merge is local and bounded (one node, fixable by split); a wrong *type* merge is a schema error that corrupts every edge using that type and is propagated by the refactor across the whole graph. So the concept layer auto-merges above 95% confidence (ADR-0001), while the ontology layer auto-merges only above 99.5% - obvious 1:1 type duplicates are filtered automatically, everything riskier goes to a human curation queue.

(An earlier, literal tolerance=0 at the schema layer was rejected as dogmatic: it would queue even 0.99-confidence trivialities for manual review, producing alert fatigue with no safety gain over 99.5%.)

## Refactor mechanics

When a curated ontology change is approved, the async refactor retags existing edges to the new vocabulary. It runs at Temperature=0 against a **pinned model snapshot** (e.g. `gpt-4-0613`), never a `-latest` alias - so retagging is deterministic *and* stable across time; the graph does not shift under the user due to background API model bumps. Temperature=0 alone is insufficient: it is deterministic only within a single model snapshot, not across version changes.

## Edge type history (event-sourced)

Retagging does not overwrite an edge's type. Each edge carries an append-only **type history**: index 0 is the LLM's original assertion (immutable), each refactor appends the migrated type, and the edge's *current type* is the projected state of the last entry - a read model off the log, not a stored field. This applies event sourcing to edge type, preserving the original assertion the way the verbatim braindump is preserved under its cleaned rendering (braindump-as-source-of-truth, extended to the edge layer).

Chosen over:
- **Overwrite** the type on retag - rejected: provenance would then claim the asserting braindump asserted a type it never did, breaking source-of-truth.
- **Original + current (two types)** - rejected: collapses on a second refactor (`affects`→`endangers`→`critically_endangers`); only an append-only log survives repeated migrations.

## Consequences

- **Refines ADR-0002:** an edge's type is now an append-only type history, not a fixed field. Edge identity anchors on the *original* asserted type (index 0) - `(source, original_type, target)` - since the current type is a mutable projection; retagging appends to an edge's history rather than deleting one edge and creating another. ADR-0002's "(source, type, target)" key is read with *original* type.
- The fractal reuse means three embedding collections now live in the vector store: braindump-embeddings (semantic search), concept-embeddings (concept identity, ADR-0001), and type-embeddings (ontology dedup, this ADR).

## Free-tier-fallback exception (issue #86)

**The pinned-model determinism guarantee above is a hard-won invariant; the exception below exists ONLY because the project runs on the Gemini free tier and would otherwise be unusable when the primary model exhausts its quota. It is a temporary, logged degradation - not a relaxation of the principle.**

When the primary text model (`GEMINI_TEXT_MODEL`, default `gemini-2.0-flash`) trips `GEMINI_FALLBACK_MAX_ATTEMPTS` (default 5) consecutive transient failures (429 quota-exceeded / 5xx / transport), a circuit breaker opens and subsequent text-generation calls - *including `generate_pinned` (the ontology refactor)* - route to a fallback model (`GEMINI_TEXT_MODEL_FALLBACK`, default `gemini-3.1-flash-lite`) until a cooldown (`GEMINI_FALLBACK_COOLDOWN_SECS`, default 3600s) expires and a half-open probe of the primary succeeds.

This means a refactor that begins against the pinned primary can, mid-flight, retag edges through a different model snapshot. That breaks the cross-time determinism the pinned-model rule exists to provide: a refactor split across the primary/fallback boundary is not reproducible against either single snapshot, and the type-history entries appended during the fallback window carry the fallback model's judgment, not the primary's.

We accept this on the free tier because the alternative is worse: a refactor blocked by a 429 stays blocked for the whole quota window, and the graph drifts further from the governed ontology the longer the refactor is stalled. A *best-effort* refactor on the fallback model is preferable to *no* refactor - the type history is append-only (the original assertion at index 0 is never overwritten), so a future re-refactor against the pinned primary can always correct a fallback-era mistag, and the degradation is observable via tracing so it is never silent.

**Restoring the invariant:** the moment a half-open probe of the primary succeeds, the circuit closes and all subsequent `generate_pinned` calls (including new refactors) run against the pinned primary again. The exception is scoped to the open-circuit window and self-heals; it is not a permanent widening of the model surface. When the project moves off the free tier, the fallback circuit can be disabled (set `GEMINI_FALLBACK_MAX_ATTEMPTS` high, or wire the primary as the fallback too) and the pinned-model guarantee returns to its uncompromised form.

Embeddings are **not** covered by the fallback - the embedding model is identity-calibrated (ADR-0001) and a fallback embed model would break concept identity and mismatch the vec0 table dimensionality. Only text generation routes through the circuit.

## Free-tier-fallback counter scoping (issue #99 addendum)

**Root cause of the circuit never opening:** the original breaker (#86) reset the consecutive-failure counter to 0 on *every* successful primary text-generation call while the circuit was closed (`record_success(false)`), with no logging. On the free tier several text-generation paths share one `FallbackLlm` - ingest cleaning/extraction, chat synthesis, retrieval, and the ontology refactor - and they interleave. In the production incident, ingest kept hitting 503s from the primary, but between ingest attempts a chat synthesis or retrieval call succeeded against the primary and silently reset the counter to 0. The counter never reached the 5-failure threshold, so the circuit never opened despite 8 consecutive transient ingest failures, and no "fallback circuit opened" (WARN) or "fallback model serving" (DEBUG) log ever appeared. The unlogged reset was the observability gap that blocked diagnosis.

**Fix - the counter is not reset by a closed-circuit success (Option C):** a successful primary call while the circuit is closed no longer touches the consecutive-failure counter. The counter only increments on a transient failure and only resets when a half-open probe of the primary succeeds and closes the circuit (`record_success(true)`). This is the most aggressive but simplest of the scoped-counter options: 5 transient failures from *any* text-gen path open the circuit regardless of interspersed successes from other paths, which is exactly the invariant the incident violated. The trade-off is over-sensitivity - once the counter has accumulated a few failures it stays sticky until the circuit opens and self-heals via the half-open probe - which is acceptable on the free tier where the primary is chronically overloaded and the cost of a false open (a brief window on the lighter fallback model) is far lower than the cost of a false closed (the primary stays wedged on 429/5xx with no failover).

**Observability (issue #99 part 1):** every consecutive-failure counter transition is now logged so the breaker state is fully auditable - increment (`debug`), threshold-reached/open (`warn`), half-open probe success/close (`info`), half-open probe failure/re-open (`warn`), and the closed-circuit success that intentionally leaves the counter unchanged (`debug`). An operator can trace exactly when the counter increments, when it is (not) reset, and when the circuit opens.

Rejected alternatives:
- **Decrement-on-success (Option B):** a success would drain the counter by 1, so sustained failures still trip the circuit *as long as failures outpace successes*. Rejected because 5 failures with even one interspersed success only reach a counter of 4 - it does not strictly satisfy the acceptance criterion that 5 transient failures open the circuit regardless of interspersed successes.
- **Per-call-site counters (Option D):** would require threading a call-site identifier through the `Llm` trait, invading every caller. Rejected as too invasive for the seam.
- **Sliding/decaying window (Option A):** rejected as unnecessary complexity; the half-open probe already provides the recovery signal.
