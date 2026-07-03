# Edges are typed, directional, and accrete provenance

Relationships between concepts are typed edges drawn from a fixed ontology (the LLM picks a type, never invents one), directional (`A —[type]→ B`), and each edge carries a provenance property listing the braindumps that asserted it. One edge per (source, type, target) *accretes* — a second braindump asserting the same edge adds its id to `asserted_by` rather than creating a new row — mirroring how concepts accrete. Deleting a braindump drops its id from every edge's provenance; an edge vanishes when its last asserter is removed.

## Considered options

- **Co-occurrence edges** (no type, just "appeared together"): rejected — produces a navigable web but edges carry no meaning; you reach a concept's neighbour without knowing *why*.
- **Edge-as-node** (relationships modelled as concept nodes): rejected — flattens the schema but inflates node count and blurs the concept/edge distinction.
- **One edge per braindump** (each assertion its own row): rejected — maximal fidelity and delete-clean, but inconsistent with concept identity (which accretes), and "the A→B relationship" becomes a query across many rows rather than a thing.

## Consequences

Two braindumps can assert contradictory edges between the same pair (e.g. `Maria`—[endangers]→`Q3` and `Maria`—[helps]→`Q3`); both coexist as separate typed edges, each with its own provenance, so contradictions are preserved rather than silently resolved. When concept merges (ADR-0001) fold two concepts into one, edges from both flow to the merged node and may surface such contradictions — which provenance makes visible rather than hiding.

_Refined by ADR-0003: an edge's type is an append-only type history (event-sourced), with current type as a projection; edge identity anchors on the original asserted type, not the current (mutable) type._

_Further refined by ADR-0006: provenance entries are origin-typed — braindump vs chat inference — so user thoughts and endorsed LLM deductions stay distinguishable._
