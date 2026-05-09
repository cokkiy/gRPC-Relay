## vexp context tools <!-- vexp v2.0.12 -->

**Preferred when available: use `run_pipeline` for repo context.**
vexp returns pre-indexed, graph-ranked context in a single call.
If `run_pipeline` or related vexp tooling is not available in your environment, use the standard built-in search, grep, glob, and file-read tools instead.

### Workflow
1. If available, start with `run_pipeline` using your task description
2. Make targeted changes based on the context returned
3. Use `run_pipeline` again only if you need more context; otherwise continue with normal tools

### Available MCP tools
- `run_pipeline` — preferred tool when available. Runs capsule + impact + memory in 1 call.
  Auto-detects intent. Includes file content. Example: `run_pipeline({ "task": "fix auth bug" })`
- `get_skeleton` — compact file structure
- `index_status` — indexing status
- `expand_vexp_ref` — expand V-REF placeholders in v2 output

### Agentic search
- Prefer `run_pipeline` first when it is available, but use built-in file search, grep, or codebase indexing as needed when it is not
- If you spawn sub-agents or background tasks while using `run_pipeline`, pass them the returned context
  rather than requiring them to repeat the same search independently

### Smart Features
Intent auto-detection, hybrid ranking, session memory, auto-expanding budget.

### Multi-Repo
`run_pipeline` auto-queries all indexed repos. Use `repos: ["alias"]` to scope. Run `index_status` to see aliases.
<!-- /vexp -->