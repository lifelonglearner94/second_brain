# Chat writes back: governed inference proposals, origin-typed by epistemic class

Chat is not only a read surface (ADR-0005) but a governed **write-back** surface. When chat surfaces a multi-hop connection the user overlooked, that inference becomes a *proposal* for a new edge — never persisted silently. The user wanted a thinking partner that grows the brain, not an amnesiac mirror; ownership is retained by routing every inference through the same fractal governance built for concepts (ADR-0001) and types (ADR-0003).

## Two proposal modes, distinct epistemic status

Not all proposals are equal. Treating a deterministic path-summary and a statistical hypothesis as the same thing would violate the spirit of strict provenance tracking (ADR-0002). Proposals are explicitly typed at the origin level:

- **Structural Inference** (`origin: structural_inference`): the LLM traces an existing multi-hop edge path and proposes a direct edge summarizing it — "the graph supports this; I'm labeling existing structure." Graph-backed, deterministic, low-risk. Evidence: a traversable edge path.
- **Thematic Inference** (`origin: thematic_inference`): the LLM observes thematic density in the current Thematic Read Model partition (ADR-0008) — concepts clustered by Louvain with no connecting edge path — and proposes a new edge bridging the gap. Not graph-backed: the evidence is a statistical hypothesis from a non-deterministic partition that won't exist tomorrow. Riskier by nature.

The explicit tag lets the HITL queue distinguish graph-backed proposals from LLM-hallucinated hypotheses: when reviewing, the user knows exactly which proposals rest on existing structure and which rest on an ephemeral Louvain snapshot.

## How an inference is governed

Both proposal modes route through the same governance stack:

- **Endpoints** resolve via concept identity (ADR-0001): the proposed concepts are embedding-matched to existing ones, auto-merging above 95%, else surfaced as merge suggestions.
- **Type** resolves via the ontology (ADR-0003): the proposed type is matched to existing types, auto-merging above 99.5%, else queued.
- **The inference-claim itself** — the assertion that this connection is real — is *always human-gated*. No auto-endorse. Endorsing an LLM deduction is the highest-stakes graph mutation (it can drift the brain toward the LLM's worldview, not the user's), so the propose→HITL→endorse pattern applies with no tolerance threshold. This is the meaning of "no silent graph pollution."

Only after endorsement does the edge persist, and its provenance records the origin as `asserted_by: [Chat_Inference_ID, mode: structural|thematic]`.

## Provenance as a shield

Provenance (ADR-0002) is now **origin-typed** at two levels: each assertion is tagged as either a human braindump or a chat inference; within chat inferences, as structural or thematic. The graph tracks three strata — user thoughts, graph-backed LLM summaries, and LLM hypotheses from ephemeral evidence. Thematic inferences additionally carry a **Thematic Snapshot** (ADR-0009) — a frozen capture of the motivating cluster's braindumps — so the ephemeral evidence is preserved as an audit trail even after the cluster dissolves; structural inferences carry no snapshot, since their evidence is the graph itself (always present). This is not just audit — it is a purge lever: because inference-origin edges are identifiable by mode, the user can retract thematic inferences alone (the riskier, less-evidenced stratum) if the brain has drifted toward the LLM's connective logic, while keeping structural inferences (which only summarized existing graph paths).

## Considered options

- **Transient (read-only)** — inferences illuminate but never persist; the graph stays purely human-authored. Rejected — turns the system into an amnesiac; every "interesting connection" evaporates unless the user re-braindumps it themselves, leaving the core value on the table.
- **Ungoverned write-back** — chat writes inferred edges directly. Rejected — the LLM's deductions silently pollute the brain and drift its worldview, indistinguishable from the user's own thoughts.
- **Unified proposal mode** (no structural/thematic split): rejected — collapses two epistemically distinct acts into one. A path-summary is deterministic and graph-backed; a gap-filling proposal from a non-deterministic Louvain partition is a statistical hypothesis. Treating them identically would defeat the provenance system's purpose and leave the HITL reviewer unable to weigh risk per-proposal.

## Consequences

- **Refines ADR-0002:** provenance entries are now origin-typed at two levels — braindump vs chat inference, and within chat inferences, structural vs thematic — so user thoughts, graph-backed summaries, and LLM hypotheses stay distinguishable.
- The fractal governance now spans three layers: concepts (ADR-0001), ontology types (ADR-0003), and chat-inferred edges (this ADR) — each with the propose→HITL→endorse pattern, tolerance calibrated to blast radius.
- **Resolved — what an inference can propose.** An inference may be a new edge between existing concepts, a new edge requiring a new *type* (→ ontology queue), or a new *concept* (→ concept queue). The "new concept" case is concretely motivated by ADR-0008: a transient cluster significant enough to persist does not gain identity at the cluster layer; it routes through governed write-back as a proposed new Concept, with its links to cluster members passing through edge/type governance (ADR-0002/0003). The fractal propose→HITL→endorse pattern applies at each layer with tolerance calibrated to blast radius.
- **Thematic Inference makes the Thematic Read Model load-bearing for growth.** The Thematic Read Model (ADR-0008) is not just visualization or Chat context — it is the system's Hypothesis Engine, the sole source of Thematic Inference proposals. Thematic proposals cannot exist without it: they are the only proposals that introduce structure not derivable from the existing graph.
- **Thematic Inference provenance carries a Thematic Snapshot (ADR-0009); Structural Inference provenance does not.** The two proposal modes differ not only in epistemic status but in provenance weight: thematic edges carry a frozen capture of the ephemeral cluster that motivated them, structural edges do not (their evidence is the graph itself, always present).
