# Concept identity via embedding match, with borderline-confirm hybrid

A second brain only forms useful clusters if the same concept accretes into one node, so every newly-extracted concept is resolved against existing ones by embedding similarity and merged when the match is confident. We chose a hybrid commit policy: high-confidence matches merge silently; borderline matches are surfaced as **merge suggestions** for the user to confirm or reject, rather than committed silently.

## Considered options

- **Pure silent** (auto-merge everything above threshold): rejected — silent wrong *fusions* are invisible until noticed by accident, and the correction (concept split) is painful because it forces hand-re-sorting of every attached braindump. The avoided ingest friction returns concentrated and hard to find at correction time.
- **Pure surface** (confirm every merge): rejected — interrupts the voice-first capture flow that is the system's reason for existing.
- **LLM-as-judge** for identity: rejected — too costly to run on every extraction.

## Consequences

The hybrid catches the ambiguous fusions that made pure-silent painful, so wrong fusions become rare — but not impossible. **Concept split** (user correction for a wrongly-fused concept) still needs to exist for the residual cases, and remains expensive when a fusion went unnoticed and accreted many braindumps.
