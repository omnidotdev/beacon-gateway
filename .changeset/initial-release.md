---
"beacon-gateway": minor
---

Lighting the beacon

- Multi-channel AI assistant gateway — voice, Discord, Telegram, Slack, and WebSocket
- Built-in sandboxed shell execution tool with timeout and PATH augmentation
- Skill management with compact prompt injection and on-demand skill reading
- Per-channel tool policy enforcement
- Memory tools for persistent, session-spanning context
- Cron scheduling for automated tasks
- Plugin extensibility via MCP
- Direct MCP server management over stdio transport with `[[mcp_servers]]` TOML config
- MCP tools automatically registered in `ToolExecutor` alongside Synapse/plugin tools
- Plugin MCP configs (`transport: "mcp-stdio"`) auto-spawned at daemon startup
- `require_auth` middleware supporting both API key and Gatekeeper JWT
- Trellis knowledge garden API client with `[ecosystem]` config section
- Browser automation tools: navigate, click, type, screenshot, extract
- BM25 keyword scorer for ranked memory search with hybrid vector+keyword matching
- 10 bundled skills: summarize, translate, code-review, explain, meeting-notes, proofread, data-analysis, email-draft, debug, and default
- `beacon setup` wizard with channel configuration, MCP server discovery, and life.json setup
- Consolidated shared modules into agent-core (loop detection, web fetch/search/readability, tool policy, skill types)
