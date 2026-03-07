# beacon-gateway

## 0.1.0

### Minor Changes

- [`4cf6759`](https://github.com/omnidotdev/beacon-gateway/commit/4cf6759c2dd6b87d366f4ff8d3792459003bb9da) Thanks [@coopbri](https://github.com/coopbri)! - Add BM25 keyword search and expand bundled skills to 10

  - BM25 scorer (`src/db/bm25.rs`) for ranked keyword matching in memory search
  - `search_keyword()` method on `MemoryRepo` replaces naive LIKE matching with BM25 ranking, falling back to LIKE for partial/substring matches
  - `search_hybrid()` now uses BM25 keyword results merged with vector similarity
  - 9 new bundled skills: summarize, translate, code-review, explain, meeting-notes, proofread, data-analysis, email-draft, debug

- [`b07b856`](https://github.com/omnidotdev/beacon-gateway/commit/b07b856bc1ae679c8f571efd6b34b34bed2d70b7) Thanks [@coopbri](https://github.com/coopbri)! - Lighting the beacon

  - Multi-channel AI assistant gateway — voice, Discord, Telegram, Slack, and WebSocket
  - Built-in sandboxed shell execution tool with timeout and PATH augmentation
  - Skill management with compact prompt injection and on-demand skill reading
  - Per-channel tool policy enforcement
  - Memory tools for persistent, session-spanning context
  - Cron scheduling for automated tasks
  - Plugin extensibility via MCP

- [`36329ac`](https://github.com/omnidotdev/beacon-gateway/commit/36329acaabc547232530e5f58c210ff1ee7a6e2e) Thanks [@coopbri](https://github.com/coopbri)! - Add direct MCP client, Gatekeeper auth middleware, and Trellis integration

  - Direct MCP server management over stdio transport (`src/mcp/`)
  - MCP tools automatically registered in `ToolExecutor` alongside Synapse/plugin tools
  - `[[mcp_servers]]` TOML config and startup in daemon
  - `require_auth` middleware supporting both API key and Gatekeeper JWT
  - `AuthIdentity` type for downstream user identification
  - `GATEKEEPER_AUTH_URL` env var for unified auth configuration
  - Trellis knowledge garden API client (`src/integrations/trellis.rs`)
  - `[ecosystem]` config section for optional service URLs

- [`f346550`](https://github.com/omnidotdev/beacon-gateway/commit/f34655052132edfb547bed36c7a08e5550829fb8) Thanks [@coopbri](https://github.com/coopbri)! - Wire MCP plugin transport and browser automation agent tools

  - Plugin MCP configs (`transport: "mcp-stdio"`) now auto-spawned alongside config-file MCP servers at daemon startup
  - New `BuiltinBrowserTools` exposes `BrowserController` as 5 LLM-callable tools: `browser_navigate`, `browser_click`, `browser_type`, `browser_screenshot`, `browser_extract`
  - Browser auto-launches on first tool call (lazy initialization)
  - `browser_screenshot` and `browser_extract` classified as read-only for safe parallel execution
