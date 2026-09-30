<div align="center">

<h1>⌨️ AhaKey Runtime</h1>
<h4>A Rust runtime for AhaKey hardware: BLE communication, device state, and Studio IPC.</h4>

<p>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rust-2024-000000?logo=rust" alt="Rust 2024 edition"></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/async-Tokio-463B69" alt="Async runtime: Tokio"></a>
  <a href="#overview"><img src="https://img.shields.io/badge/hardware-AhaKey_X1-2563EB" alt="Hardware: AhaKey X1"></a>
  <a href="#development"><img src="https://img.shields.io/badge/status-in_development-E9A23B" alt="Status: in development"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache_2.0-22C55E" alt="License: Apache 2.0"></a>
</p>

🌐 [English](README.md) | 🇨🇳 [简体中文](docs/zh/README.md)

[Overview](#overview) · [Quick Start](#quick-start) · [Development](#development) · [Contributing](#contributing)

</div>

## Overview

An independently running backend for **AhaKey Studio's `Studio Frontend` target**. This initial version implements Studio dev's seven phase-one WebSocket/JSON-RPC methods, with one concrete Rust struct per hardware model. `ModelX1` is the first real adapter; `MockX1` is an explicit simulator for integration testing.

| Component | Implemented scope |
| --- | --- |
| Device discovery | Connected-peripheral retrieval plus service-filtered active BLE scanning; one selected X1 |
| Hardware adapter | GATT connect, pushed mode/status and disconnect events, serialized command/ACK exchanges, timeout session reset and reconnect |
| Configuration | Four modes, shortcuts/macros/disabled keys, ASCII descriptions, nine state-to-light mappings, global brightness |
| IPC | Loopback WebSocket, private discovery/token, authentication, atomic snapshot subscriptions, bounded clients/events |
| Operations | Durable acceptance, per-device reservation, idempotency, progress, terminal records, restart recovery |
| Simulator | `--mock`, separate journal, actual Rust IPC and scheduler without Bluetooth |

Validated on macOS with Rust 1.98.1, automated tests and the **unmodified Studio dev Swift IPC client**. Real X1 discovery, status reads and BLE disconnect/reconnect have also been verified on one macOS device. **Real configuration writes and power-cycle persistence have not been validated.** Windows/Linux are not yet validated. This version does not implement OLED resources, previews, Hook integration, voice, firmware upgrades or a service installer.

## Quick Start

For firmware integration, see the [BLE addresses, architecture and parameter guide (中文)](docs/ble-integration-guide.md).

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

The current Runtime does not implement configuration readback (newer firmware adds `0x9D`, which is not yet integrated). A real device therefore initially reports `configuration: null`; Studio correctly disables full-configuration saving. To establish a baseline, supply an **explicit complete configuration** (all four modes and brightness) and select the exact device:

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

ACK-confirmed configuration is valid only for the current uninterrupted device session. Disconnect, restart or observed external brightness changes invalidate the baseline. Reinitialization is explicit; no cached configuration is automatically written to hardware. Save ACKs do not prove persistence across power loss.

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
├── main.rs                # CLI, process lifecycle, signals
├── lib.rs                 # Library entry
├── ipc.rs                 # Authenticated JSON-RPC and ordered subscriptions
├── wire.rs                # Studio-facing projections
├── configuration.rs       # Strict business types, validation and scoped merging
├── runtime.rs             # Device workers, admission, state and operation lifecycle
├── storage.rs             # Atomic durable journal and crash recovery
├── discovery.rs           # Discovery permissions and instance lock
├── ahakey/
│   ├── device.rs          # AhaKey container for Box<dyn Hardware>
│   ├── hardware.rs        # Model-independent hardware interface
│   ├── model_x1.rs        # Concrete X1 session and confirmation behavior
│   ├── mock.rs            # Explicit X1 simulator
│   └── state.rs           # Shared status and X1 status decoding
└── protocol/
    ├── ble.rs             # Discovery and model selection
    └── x1.rs              # X1-only command encoding
contracts/runtime-v1/      # Studio dev schema and unchanged fixtures
```

A new model implements `Hardware` in its own struct and supplies its own discovery matcher, configuration encoder, status decoding and confirmation rules. Neither Studio IPC nor the scheduler encodes X1 bytes. See [initial architecture and behavior](docs/runtime-v1.md) for invariants, errors, limits and hardware acceptance checks.

The journal retains up to **128 accepted operation IDs** without silently evicting old results; new writes then return `BUSY`. This is an explicit initial-version bound. Before archive/reset maintenance, stop Runtime and reconcile pending client operations. Records live beside discovery as `discovery.state.json` (hardware) or `discovery.mock-state.json` (simulator). Retained configurations are audit history, not current hardware evidence.

## Contributing

Contributions to device support, protocol handling, tests and documentation are welcome. Use [Issues](https://github.com/ZephyrKeXiner/ahakey-runtime-rust/issues) and [Pull Requests](https://github.com/ZephyrKeXiner/ahakey-runtime-rust/pulls).

For hardware reports include OS, Rust version, device/firmware model, reproduction steps and relevant logs with unnecessary device identifiers removed. Run the development checks and distinguish automated validation from hardware observations. Keep this README and its Chinese translation aligned.

<p align="center">
  <a href="https://github.com/ZephyrKeXiner/ahakey-runtime-rust/graphs/contributors">
    <img src="https://contrib.rocks/image?repo=ZephyrKeXiner/ahakey-runtime-rust" alt="AhaKey Runtime contributors">
  </a>
</p>

## License

[Apache License 2.0](LICENSE).

## Runtime key remapping

`keymap.apply` writes selected keys without a complete configuration baseline or lighting support. It uses the existing device worker and durable operations. See [API and IPC client](docs/keymap-api.md).
