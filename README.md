<div align="center">

# Beacon Gateway

Core voice and messaging gateway daemon

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE.md)

</div>

## Overview

Beacon Gateway is the always-on Rust daemon that powers voice and messaging for AI assistants. It handles wake word detection, speech processing, messaging channel adapters, persona management, and agent integration.

## Features

- **Voice Processing** - Wake word detection, STT (Whisper), TTS (OpenAI/ElevenLabs)
- **Messaging Channels** - Discord, Slack, WhatsApp, Telegram, Signal, Teams, Matrix, Google Chat, iMessage
- **Agent Integration** - Uses Omni CLI as the intelligence layer
- **Persona Management** - Configurable assistants via [persona.json](https://persona.omni.dev)
- **Device Identity** - Ed25519 keypair-based authentication
- **Local-First** - All data stays on your machine

## Prerequisites

- [Rust](https://rustup.rs) 1.85+
- API keys (see `.env.local.template` in metarepo)

## Development

```bash
# From metarepo root
tilt up

# Or directly
cargo build
cargo run -- --persona orin --foreground -v
```

## Testing

```bash
cargo test
```

## Docker

```bash
docker build -t beacon-gateway .
docker run beacon-gateway
```

## Ecosystem

- **[Omni CLI](https://github.com/omnidotdev/cli)**: Agentic CLI that powers Beacon's intelligence layer
- **[Omni Terminal](https://github.com/omnidotdev/terminal)**: GPU-accelerated terminal emulator built to run everywhere

## License

The code in this repository is licensed under MIT, &copy; [Omni LLC](https://omni.dev). See [LICENSE.md](LICENSE.md) for more information.
