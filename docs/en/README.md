<div align="center">

<h1>⌨️ AhaKey Runtime</h1>
<h4>A Rust runtime for AhaKey hardware: BLE communication, device state, and Studio IPC.</h4>

<p>
  <a href="../../Cargo.toml"><img src="https://img.shields.io/badge/Rust-2024-000000?logo=rust" alt="Rust 2024 edition"></a>
  <a href="../../Cargo.toml"><img src="https://img.shields.io/badge/async-Tokio-463B69" alt="Async runtime: Tokio"></a>
  <a href="#overview"><img src="https://img.shields.io/badge/hardware-AhaKey_X1-2563EB" alt="Hardware: AhaKey X1"></a>
  <a href="#development"><img src="https://img.shields.io/badge/status-in_development-E9A23B" alt="Status: in development"></a>
  <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-Apache_2.0-22C55E" alt="License: Apache 2.0"></a>
</p>

🇨🇳 [简体中文](../../README.md) | 🌐 English

[Overview](#overview) · [Quick Start](#quick-start) · [Development](#development) · [Contributing](#contributing)

</div>

## Overview

An independently running backend for **AhaKey Studio's `Studio Frontend` target**. This initial version implements Studio dev's seven phase-one WebSocket/JSON-RPC methods, with one concrete Rust struct per hardware model. `ModelX1` is the first real adapter; `MockX1` is an explicit simulator for integration testing.

| Component | Implemented scope |
| --- | --- |
| Device discovery | Connected-peripheral retrieval plus service-filtered active BLE scanning; one selected X1 |
| Hardware adapter | GATT connect, pushed mode/status and disconnect events, serialized command/ACK exchanges, timeout session reset and reconnect |
| Configuration | Four-mode writes and automatic device readback of key actions, descriptions, brightness and light mappings |
| OLED | CP2 PNG/JPEG/GIF staging, resource metadata, confirmed uploads, pause and explicit resume |
| IPC | Loopback WebSocket, private discovery/token, authentication, atomic snapshot subscriptions, bounded clients/events |
| Operations | Durable acceptance, per-device reservation, idempotency, progress, terminal records, restart recovery |
| Simulator | `--mock`, separate journal, actual Rust IPC and scheduler without Bluetooth |

Validated on macOS with Rust 1.98.1, automated tests and the **unmodified Studio dev Swift IPC client**. Real X1 discovery, status reads and BLE disconnect/reconnect have also been verified on one macOS device. **Real configuration writes and power-cycle persistence have not been validated.** Windows/Linux are not yet validated. OLED uploads are implemented for the CP2 resource profile; real uploads, display orientation and power-cycle persistence remain unvalidated. This version does not implement previews, Hook integration, voice, firmware upgrades or a service installer.

## Quick Start

For application developers, start with the [WebSocket API reference (中文)](../websocket-api.md): discovery, authentication, all RPC methods, events, data models, error recovery, and a runnable read-only client.

For firmware integration, see the [BLE addresses, architecture and parameter guide (中文)](../ble-integration-guide.md).

For image upload, see the [OLED API](../oled-api.md) and [Node.js upload example](../../examples/oled-upload.mjs).

Use a Rust toolchain supporting edition 2024. Development currently targets macOS; no minimum supported toolchain is declared yet.

```sh
cargo build --locked
cargo run --locked -- --mock
```

The simulator publishes discovery at:

```text
~/Library/Application Support/AhaKey/runtime/discovery.json
```

In **AhaKey Studio's `dev` branch**, open `AhaKey Studio.xcodeproj`, select **Studio Frontend → My Mac**, and press **⌘R**. The real IPC client connects to this Rust simulator. The **Studio Frontend Mock** scheme instead uses Swift's own in-memory mock and does not contact Rust. The older **AhaKey Studio** scheme runs the legacy device owner.

For a separate test instance:

```sh
cargo run --locked -- --mock --discovery "$HOME/.ahakey-runtime-test/discovery.json"
```

Set `AHAKEY_RUNTIME_DISCOVERY` to that same absolute file path in Xcode's **Edit Scheme → Run → Arguments → Environment Variables**. The discovery directory must be private (`0700` on Unix); files are written atomically with `0600`. No token is printed to the terminal. Ctrl+C or SIGTERM removes discovery and gives accepted operations up to 30 seconds to finish.

### Real hardware and first configuration

Stop the legacy Studio/Agent BLE owner, then run:

```sh
cargo run --locked
```

Enable Bluetooth and grant the hosting terminal Bluetooth access. Discovery logs stable IDs such as `x1-<platform-peripheral-id>`. Use `--device-id` to select a specific device. Missing adapters/devices are retried while IPC remains available.

Runtime targets remote [Fireware-harness `076382b`](https://github.com/AhakeyAI/AhaKey-X1-hardware-source/tree/076382b2214289ac7bc1735d669d7de78730c4da). It identifies the catalog with `0x9F`, reads key actions/descriptions with `0x87`, and brightness/light mappings with `0x88`. Connect, reconnect and process restart trigger readback; two matching collections are published through `configuration.get.readback`.

When all writable fields are readable and representable, Runtime establishes a complete session configuration baseline automatically. Oversized or unrepresentable keys stay unknown. Independent key remapping remains available with partial readback. To explicitly establish a full baseline, supply a **complete configuration** (all four modes and brightness) and select the exact device:

```sh
cargo run --locked -- --device-id 'x1-REPLACE_WITH_DISCOVERED_ID' --initialize /absolute/path/configuration.json
```

The file must contain a `Configuration` object (`device` and `modes`), not a document or RPC envelope. This deliberately overwrites the specified device's key/light configuration, waits for every ACK including save, and verifies brightness readback. It runs once after the first successful connection. An example can be prepared from the vendored fixture, then edited before use:

```sh
python3 - <<'PY'
import json
from pathlib import Path
source = json.loads(Path('contracts/runtime-v1/fixtures/configuration.json').read_text())
Path('initial-configuration.json').write_text(json.dumps(source['configuration'], indent=2) + '\n')
PY
```

Disconnect, restart or observed external brightness changes invalidate the baseline and readback. Reconnection reads the device again. No cached configuration is automatically written to hardware. This firmware has no task light mode query, so `readback.device.taskLightMode` is `null`. Readback reflects live RAM, and save ACKs do not prove persistence across power loss.

## Development

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 scripts/check-studio-client.py --studio '../AhaKey Studio'
```

The final command requires macOS, Swift, and a local Studio `dev` branch. It extracts the original three Swift runtime source files without changing Studio's checkout, compiles them with a smoke harness, launches an isolated Rust simulator, and tests the actual client over WebSocket. Temporary processes and files are cleaned up.

```text
src/
├── main.rs              # Executable entry point
├── app/                 # CLI, bootstrap and process shutdown
├── runtime/             # Device ownership and orchestration
│   ├── state.rs         # Snapshots, events and input state
│   ├── device.rs        # Connection intent and observed device state
│   ├── worker.rs        # Exclusive hardware loop and reconnect/readback
│   ├── configuration.rs # Configuration/keymap execution
│   ├── oled.rs          # Device uploads and bounded recovery
│   └── operations.rs    # Durable admission, idempotency, pause/resume
├── configuration/       # Configuration, actions, readback and keymap models
├── oled/                # Image codec, integrity, resources and binary transfers
├── ipc/                 # Listener, session, authentication and RPC dispatch
├── ahakey/
│   ├── hardware.rs      # Hardware interface
│   ├── mock.rs          # Explicit simulated X1 adapter
│   └── x1/              # X1 connection, commands, notifications and resources
├── input/               # HID state and native-helper supervision
├── storage/             # Journal recovery and atomic filesystem writes
├── operation.rs         # Mutation identity and operation result model
├── wire.rs              # Studio-facing DTOs and advertised methods
├── protocol/ble.rs      # BLE discovery and model selection
├── discovery.rs         # Private discovery file and instance lock
├── debug.rs             # Bounded diagnostics
└── error.rs             # Shared error codes
contracts/runtime-v1/    # Unchanged Studio contract and fixtures
tests/runtime/          # Tests grouped by feature, with shared support
```

See [code layout and ownership](../code-layout.md) for module responsibilities and public-path compatibility.

A new model implements `Hardware` in its own struct and supplies its own discovery matcher, configuration encoder, status decoding and confirmation rules. Neither Studio IPC nor the scheduler encodes X1 bytes. See [initial architecture and behavior](../runtime-v1.md) for invariants, errors, limits and hardware acceptance checks.

The journal retains up to **128 accepted operation IDs** without silently evicting old results; new writes then return `BUSY`. This is an explicit initial-version bound. Before archive/reset maintenance, stop Runtime and reconcile pending client operations. Records live beside discovery as `discovery.state.json` (hardware) or `discovery.mock-state.json` (simulator). Retained configurations are audit history, not current hardware evidence.

## Contributing

Contributions to device support, protocol handling, tests and documentation are welcome. Use [Issues](https://github.com/ZephyrKeXiner/ahakey-runtime-rust/issues) and [Pull Requests](https://github.com/ZephyrKeXiner/ahakey-runtime-rust/pulls).

For hardware reports include OS, Rust version, device/firmware model, reproduction steps and relevant logs with unnecessary device identifiers removed. Run the development checks and distinguish automated validation from hardware observations. Keep this README and the main Chinese README aligned.

<p align="center">
  <a href="https://github.com/ZephyrKeXiner/ahakey-runtime-rust/graphs/contributors">
    <img src="https://contrib.rocks/image?repo=ZephyrKeXiner/ahakey-runtime-rust" alt="AhaKey Runtime contributors">
  </a>
</p>

## License

[Apache License 2.0](../../LICENSE).

## Runtime key remapping

`keymap.apply` writes selected keys without a complete configuration baseline or lighting support. It uses the existing device worker and durable operations. See [API and IPC client](../keymap-api.md).
