# Ingest fires on explicit submit; braindumps are immutable thought-snapshots, edits are error-correction only

Ingest (cleaning + extraction + embedding + identity-resolution, ADR-0001/0002/0003) fires on **explicit submit** — the user speaks, transcription streams into a hybrid text field, the user fixes it, then submits. The committed "verbatim" is the *user-confirmed* text (post-edit, pre-clean); raw STT is a capture artifact, not the source of truth. (Auto-submit-on-silence was rejected: it ingests potentially-garbled STT and makes the re-extraction/provenance cascade a hot path rather than an edge case.)

## Braindumps are immutable thought-snapshots; edits are error-correction only

A braindump is an immutable snapshot of the user's state of mind at a timestamp. Substantive thinking-evolution is never retroactive — if you realize `Mark` is the problem, not `Maria`, you do not edit the old braindump; you submit a new one, and the graph grows additively (new braindump, new edges). The brain's temporal history is carried by the *additive stream of immutable braindumps* plus accreting concepts/edges — not by per-text versioning.

The edit function exists solely to fix administrative garbage (STT hallucinations, fat-finger typos). Edits overwrite the text **in place**; re-extraction fires on the corrected text, mutating derived concepts/edges and cascading through provenance (per ADR-0002) — but this is rare and bounded, because Model A keeps post-submit edits infrequent and typo-fixes rarely change the intended thought (a typo restored to its intended word *restores* the intended semantic, it doesn't revise it).

## Why overwrite here but event-source elsewhere

This is the one place in the model where text is overwritten rather than event-sourced — and it is conscious. The governing principle: **event-source substantive evolution; overwrite non-substantive error-correction.** The ontology is event-sourced (`type_history`, ADR-0003) because schema evolution is substantive and its lineage is worth preserving. Braindump text is overwritten because typo correction is not substantive — there is zero historical value in the lineage of a typo. Per-text versioning (`text_history`) was rejected as redundant: the additive braindump stream already provides the temporal history it would duplicate.

## Considered options

- **Auto-submit on silence (Model B):** rejected — frictionless but ingests garbled STT and makes the re-extraction + provenance cascade a frequent hot path; the heaviest pipeline runs on most braindumps.
- **`text_history` (versioned braindump text, fractal reuse of `type_history`):** rejected — preserves typo-lineage with zero value, and duplicates the temporal history the additive stream already carries. Event-sourcing is for substantive change, not capture defects.
- **No edits allowed (fully immutable text):** rejected — STT is noisy enough that typo-correction is a necessary escape hatch; forbidding edits would poison the graph with unfixable capture garbage.

## Consequences

- The graph is semantically append-only: you never rewrite a past thought, only add new braindumps. The "diary over time" emerges from accumulation, not versioning.
- Re-extraction on edit mutates derived concepts/edges and cascades provenance (ADR-0002), but is rare and bounded — an edge case, not a hot path, because of Model A + edits-are-typo-fixes.
- Refines the Braindump glossary: a braindump is an immutable thought-snapshot; "verbatim" is the user-confirmed text at submit (overwritable only for error-correction).
