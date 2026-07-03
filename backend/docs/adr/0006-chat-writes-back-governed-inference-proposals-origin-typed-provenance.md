# Chat writes back: inferences become governed edge proposals, provenance-tagged by origin

Chat is not only a read surface (ADR-0005) but a governed **write-back** surface. When chat surfaces a multi-hop connection the user overlooked, that inference becomes a *proposal* for a new edge — never persisted silently. The user wanted a thinking partner that grows the brain, not an amnesiac mirror; ownership is retained by routing every inference through the same fractal governance built for concepts (ADR-0001) and types (ADR-0003).

## How an inference is governed

A chat-proposed edge is not committed directly. It routes through the existing governance stack:

- **Endpoints** resolve via concept identity (ADR-0001): the proposed concepts are embedding-matched to existing ones, auto-merging above 95%, else surfaced as merge suggestions.
- **Type** resolves via the ontology (ADR-0003): the proposed type is matched to existing types, auto-merging above 99.5%, else queued.
- **The inference-claim itself** — the assertion that this connection is real — is *always human-gated*. No auto-endorse. Endorsing an LLM deduction is the highest-stakes graph mutation (it can drift the brain toward the LLM's worldview, not the user's), so the propose→HITL→endorse pattern applies with no tolerance threshold. This is the meaning of "no silent graph pollution."

Only after endorsement does the edge persist, and its provenance records the origin as `asserted_by: [Chat_Inference_ID]`.

## Provenance as a shield

Provenance (ADR-0002) is now **origin-typed**: each assertion is tagged as either a human braindump or a chat inference. The graph tracks the difference between the user's raw thoughts and their endorsed LLM deductions. This is not just audit — it is a purge lever: because inference-origin edges are identifiable, the user can (in principle) retract all chat-inferred edges at once if the brain has drifted toward the LLM's connective logic.

## Considered options

- **Transient (read-only)** — inferences illuminate but never persist; the graph stays purely human-authored. Rejected — turns the system into an amnesiac; every "interesting connection" evaporates unless the user re-braindumps it themselves, leaving the core value on the table.
- **Ungoverned write-back** — chat writes inferred edges directly. Rejected — the LLM's deductions silently pollute the brain and drift its worldview, indistinguishable from the user's own thoughts.

## Consequences

- **Refines ADR-0002:** provenance entries are now origin-typed (braindump vs chat inference), not a flat braindump-id list.
- The fractal governance now spans three layers: concepts (ADR-0001), ontology types (ADR-0003), and chat-inferred edges (this ADR) — each with the propose→HITL→endorse pattern, tolerance calibrated to blast radius.
- **Brain-drift risk remains.** Even with every inference human-gated, the *proposal space* is LLM-curated: the user only ever sees connections the LLM thought to draw. Provenance origin-typing (and the purge lever) is the mitigation, not a cure.
- **Open — what an inference can propose.** An inference may be a new edge between existing concepts, a new edge requiring a new *type* (→ ontology queue), or even a new *concept* (→ concept queue). The fan-out across governance layers is implied but not yet fully specified.
