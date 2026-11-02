# voxel-approved — Memory Protocol

This workspace is indexed in the local MemPalace palace (MCP server `mempalace`,
47 tools, wing **`voxel_approved`**, palace `~/.mempalace/palace`). Full project
instructions live in `../AGENTS.md`; the parts that apply here:

1. **Search before asserting.** For past decisions, root causes, "why did we…",
   prior review findings, or "last time…", call `mempalace_search`
   (optionally `wing: "voxel_approved"`) instead of guessing or re-reading the tree.
2. **Look up code** with `mempalace_code_search` / `mempalace_file_context` before
   falling back to grep + read across this 58 GB workspace.
3. **File durable knowledge** (non-obvious fix, design choice, rejected
   alternative) with `mempalace_add_drawer`; check `mempalace_check_duplicate` first.
   Changing facts (versions, statuses) go to the knowledge graph:
   `mempalace_kg_invalidate` then `mempalace_kg_add`.
4. **End the session** with one `mempalace_diary_write` entry (what changed, what
   is open) so the next session can `mempalace_diary_read`.

Refresh the index after large moves:

```bash
mempalace mine . --wing voxel_approved --direct
```
