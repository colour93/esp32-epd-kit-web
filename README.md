# EPD Agent and Device Workbench

Cross-platform Rust Agent and embedded React workbench for ESP32 E-Paper BLE v3.

```text
Browser ──127.0.0.1 HTTP/SSE──> Rust Agent ──BLE v3──> ESP32
                                    │
                                    └──stdio JSONL──> codex app-server
```

The Agent is the only BLE owner. React does not use Web Bluetooth and never calls a remote service. Codex usage is read from the existing local Codex login through the official app-server stdio protocol.

After authentication the Agent saves the target device ID and stable `EPD-KIT-*` name in its user configuration directory. Automatic discovery reconnects to that target instead of choosing the strongest signal. If a platform changes the device ID, the name is used only when it identifies one candidate; a new installation with multiple candidates requires manual selection.

For an owned battery-profile device, each wake is a short connection window: the Agent reconnects, pushes current data, calls `system.sync.complete`, and lets firmware return to deep sleep. BLE advertising contains discovery flags only, not the resource payload.

The production port is `38473`. `--port <port>` or `EPD_AGENT_PORT` may select another loopback port for local development.

## Components

- `agent/src/ble.rs`: btleplug discovery, pairing, MTU-aware MessagePack framing, reconnection, and BLE command serialization.
- `agent/src/codex.rs`: app-server lifecycle, account state, rate-limit polling/notifications, projection, and retry backoff.
- `agent/src/web.rs`: loopback-only Axum API, SSE, strict local session, and embedded assets.
- `agent/src/tray.rs`: management page, pause/resume, and exit tray menu.
- `src/`: overview, hardware/power, resource/renderer, Codex, trusted-host, and diagnostics views.

## Development

Install and build the Web application with Bun:

```bash
bun install
bun run build
```

Run the Agent with the embedded production build:

```bash
bun run dev:agent
```

On macOS this command builds and signs `agent/target/debug/EPD Agent.app`, then launches it through LaunchServices. Bluetooth permission appears under **System Settings > Privacy & Security > Bluetooth** as `EPD Agent`.

For React hot reload, keep the Agent running and start Vite separately:

```bash
bun run dev
```

Vite proxies `/api` to `http://127.0.0.1:38473`. Open the tokenized URL printed by the Agent and replace its origin with the Vite origin when a new local session is needed.

## Packaging

The Rust build script runs the Bun production build and embeds all files from `dist/` into the binary.

```bash
bun run build:agent
cargo install cargo-packager
bun run package:agent
```

Packaging runs on the target operating system. Autostart uses a per-user LaunchAgent on macOS and HKCU Run on Windows.

## Local security

- listener fixed to `127.0.0.1:38473`;
- no CORS;
- loopback Origin validation on mutations;
- installation secret in a private file under the user configuration directory;
- one-time URL-fragment exchange for an `HttpOnly; SameSite=Strict` cookie;
- no Codex login/logout or token API;
- no browser access to BLE or Codex stdio.

See [BLE Protocol v3](../esp32-epd-kit/docs/ble_protocol_v3.md) and [Codex Agent architecture](../esp32-epd-kit/docs/openai_codex_usage.md).
