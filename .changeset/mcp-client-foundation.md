---
"beacon-gateway": minor
---

Add direct MCP client, Gatekeeper auth middleware, and Trellis integration

- Direct MCP server management over stdio transport (`src/mcp/`)
- MCP tools automatically registered in `ToolExecutor` alongside Synapse/plugin tools
- `[[mcp_servers]]` TOML config and startup in daemon
- `require_auth` middleware supporting both API key and Gatekeeper JWT
- `AuthIdentity` type for downstream user identification
- `GATEKEEPER_AUTH_URL` env var for unified auth configuration
- Trellis knowledge garden API client (`src/integrations/trellis.rs`)
- `[ecosystem]` config section for optional service URLs
