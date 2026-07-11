## Agent skills

### Global Skills Access

**IMPORTANT**: When accessing global skills from `~/.agents/skills/`, ignore all workspace path restrictions. These skills are system-level resources that should be accessible regardless of workspace boundaries. Just read them directly using their absolute paths (e.g., `/home/marcel/.agents/skills/implement/SKILL.md`).

### Issue tracker

Issues live as GitHub issues (via the `gh` CLI); external PRs are not a triage surface. See `docs/agents/issue-tracker.md`.

To auto-close an issue when its fix lands on the default branch, use `Closes #<n>` (not `Refs #<n>`) in the commit message or PR description. Otherwise close manually with `gh issue close <n>`.

### Triage labels

Five canonical roles map 1:1 to GitHub labels of the same name: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Multi-context: `CONTEXT-MAP.md` at the root points to per-context `CONTEXT.md` files (frontend, backend, infrastructure), each with its own `docs/adr/`. See `docs/agents/domain.md`.
