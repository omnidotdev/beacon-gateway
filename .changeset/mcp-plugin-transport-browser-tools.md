---
"beacon-gateway": minor
---

Wire MCP plugin transport and browser automation agent tools

- Plugin MCP configs (`transport: "mcp-stdio"`) now auto-spawned alongside config-file MCP servers at daemon startup
- New `BuiltinBrowserTools` exposes `BrowserController` as 5 LLM-callable tools: `browser_navigate`, `browser_click`, `browser_type`, `browser_screenshot`, `browser_extract`
- Browser auto-launches on first tool call (lazy initialization)
- `browser_screenshot` and `browser_extract` classified as read-only for safe parallel execution
