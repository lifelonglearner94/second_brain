# Retrieval is seed-then-expand: the graph is load-bearing, vectors are seed and backfill

The read path that answers a query is **seed-then-expand**, not parallel-retrieve-and-rank. Vector search over concept-embeddings finds the entry concept(s) from the query (the *seed*); the graph then traverses along typed edges to expand the neighbourhood; braindumps collected from the traversed subgraph - plus braindump-embedding backfill for strays the graph missed - form the context handed to chat. The graph controls the retrieval path; vectors are merely seed and backfill.

## Why the graph is load-bearing

The whole point of the typed-edge/ontology/provenance machinery (ADR-0001–0003) is to make *invisible* connections explicitly navigable - relationships that are structural, not lexical. Canonical case: a braindump saying *"Maria leaving tanks the timeline"* is graph-linked `Maria`-[endangers]→`Q3 launch` but never contains the word "Q3." Seed-then-expand finds it by traversing from the `Q3 launch` seed to `Maria` and collecting her braindumps. Parallel-rank does not - vector search over braindumps won't surface a braindump that doesn't lexically/semantically match the query.

## Considered options

- **Parallel-retrieve-then-rank** (vector pulls a braindump set, graph pulls a braindump set, rank-merge the two): rejected - it regresses to pure vector search with the graph as a fancy dashboard gimmick. Typed edges barely shape the result and the three ADRs of graph machinery become ornamental at read time; the vector store does the heavy lifting and the typed graph earns none of its complexity.

## Consequences

- The three embedding collections each have a distinct retrieval role: concept-embeddings (seed), the typed graph (expand), braindump-embeddings (backfill). Type-embeddings (ADR-0003) remain a write-side/ontology concern, not in the retrieval path.
- **No-seed fallback (accepted).** Seed-then-expand requires a concept seed. A query with no concept anchor (*"what's been on my mind lately?"*) cannot seed and cannot expand; it falls back to braindump-vector retrieval directly - the one place vector becomes primary rather than seed/backfill. Accepted: vector-direct for unanchored queries is fine; the graph stays load-bearing for concept-anchored queries, which is the case that justifies the typed-edge machinery.
- Chat synthesizes its answer over the retrieved braindumps (the cleaned form, consistent with the default view). Chat's full contract is still open.
