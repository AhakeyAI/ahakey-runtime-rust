<div align="center">

<h1>⌨️ AhaKey Runtime</h1>
<h4>AhaKey 硬件的 Rust 后台：BLE 通信、设备状态与 Studio IPC。</h4>

<p>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rust-2024-000000?logo=rust" alt="Rust 2024 edition"></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/async-Tokio-463B69" alt="Async runtime: Tokio"></a>
  <a href="#概述"><img src="https://img.shields.io/badge/hardware-AhaKey_X1-2563EB" alt="Hardware: AhaKey X1"></a>
  <a href="#开发"><img src="https://img.shields.io/badge/status-in_development-E9A23B" alt="Status: in development"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache_2.0-22C55E" alt="License: Apache 2.0"></a>
</p>

🇨🇳 简体中文 | 🌐 [English](docs/en/README.md)

[概述](#概述) · [快速启动](#快速启动) · [开发](#开发) · [贡献与许可](#贡献与许可)

</div>

## 概述

独立运行的 Rust 后台，对接 **AhaKey Studio dev 分支的 Studio Frontend target**。初版实现前端所需的七个 WebSocket / JSON-RPC 接口，采用“每个型号一个结构体”：`ModelX1` 实现真实 X1，`MockX1` 用于明确的模拟联调。

| 模块 | 已实现范围 |
| --- | --- |
| 发现 | 查询系统已连接外设，加按服务过滤的主动扫描；首期选定一台 X1 |
| 硬件 | GATT 连接、模式/状态及断线事件推送、串行命令/ACK、超时后重建会话及重连 |
| 配置 | 四种模式、快捷键/宏/禁用、ASCII 描述、九种状态灯效映射、全局亮度 |
| IPC | loopback WebSocket、私有 discovery/token、认证、一致快照订阅、有界连接和事件 |
| 操作 | 持久化受理、每设备独占、幂等、进度、终态与重启恢复 |
| 模拟器 | `--mock`，独立日志，实际经过 Rust IPC 与调度器，不使用蓝牙 |

OLED 图片与 GIF 上传见 [接口文档](docs/oled-api.md) 和 [Node.js 示例](examples/oled-upload.mjs)。

## 快速启动

下载程序与自动发布流程见[打包和发布说明](docs/releasing.md)。

应用开发请先看 [WebSocket 应用开发参考](docs/websocket-api.md)，包含发现、认证、全部 RPC 方法、事件、数据模型、错误恢复和可运行的只读客户端。

与新固件联调请先看 [蓝牙地址、内部架构与参数联调手册](docs/ble-integration-guide.md)。

```sh
cargo build --locked
cargo run --locked -- --mock
```

默认 discovery 文件：

```text
~/Library/Application Support/AhaKey/runtime/discovery.json
```

Studio 切换到 `dev`，用 Xcode 打开 `AhaKey Studio.xcodeproj`，选择 **Studio Frontend → My Mac**，按 **⌘R**。这个真实 IPC 客户端会连接 Rust 模拟器。

**Studio Frontend Mock** 使用 Swift 自己的内存模拟器，不连接 Rust；**AhaKey Studio** 是旧版完整客户端，仍拥有原有蓝牙后台。

独立测试目录：

```sh
cargo run --locked -- --mock --discovery "$HOME/.ahakey-runtime-test/discovery.json"
```

## 真机与首次初始化

停止旧版 Studio / Agent 对蓝牙的占用，然后运行：

```sh
cargo run --locked
```

开启蓝牙，允许承载进程的终端访问蓝牙。日志会打印稳定设备 ID：`x1-<平台外设 ID>`。可以通过 `--device-id` 固定选择；未发现设备时后台重试，IPC 仍可连接。

Runtime 以远端 [Fireware-harness `076382b`](https://github.com/AhakeyAI/AhaKey-X1-hardware-source/tree/076382b2214289ac7bc1735d669d7de78730c4da) 为固件基准。通过 `0x9F` catalog 识别设备，使用 `0x87` 读取按键动作与描述、`0x88` 读取亮度和灯效。首次连接、重连和进程重启后自动读取，两次采集一致后通过 `configuration.get.readback` 提供。

所有可写字段均可读取且能表示时，Runtime 自动建立完整 `configuration` 基线；超长按键或不可表示的数据保留未知。读回不完整时仍可独立改键；需要显式建立完整基线时，提供**四个模式及全局亮度的完整配置**并指定设备：

```sh
cargo run --locked -- --device-id 'x1-替换为发现的ID' --initialize /绝对路径/configuration.json
```

可先从协议样例提取一份，再自行修改后执行初始化：

```sh
python3 - <<'PY'
import json
from pathlib import Path
source = json.loads(Path('contracts/runtime-v1/fixtures/configuration.json').read_text())
Path('initial-configuration.json').write_text(json.dumps(source['configuration'], indent=2) + '\n')
PY
```

## 开发

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 scripts/check-studio-client.py --studio '../AhaKey Studio'
```

| 文件 | 职责 |
| --- | --- |
| `src/main.rs` / `app/` | 启动入口、参数解析、信号与退出 |
| `src/ipc/` / `wire.rs` | WebSocket 收发、认证、订阅、RPC 分发与对外数据 |
| `src/configuration/` | 配置、动作、读回和独立改键模型 |
| `src/runtime/` | 设备所有权、工作循环、配置和图片操作调度 |
| `src/oled/` | 图片解码、RGB565 编码、本地资源和二进制文件接收 |
| `src/operation.rs` | 操作请求身份、状态及幂等比较 |
| `src/storage/` / `discovery.rs` | 持久日志、崩溃恢复、私有发现文件和实例锁 |
| `src/ahakey/` | 设备容器、Hardware 接口、模拟器和公共状态 |
| `src/ahakey/x1/` | X1 连接、命令编码、通知、配置读回与图片协议 |
| `src/input/` | HID 状态模型和原生 helper 进程管理 |
| `src/protocol/ble.rs` | 设备发现与型号选择 |
| `tests/runtime/` | 按 IPC、配置、图片、改键、事件、存储分组的测试 |

新型号新增自己的结构体，实现 `Hardware`，提供匹配规则、配置编码、状态解析和确认条件。IPC 和调度器不编码 X1 字节。详细约束与真机验收见 [初版架构](docs/runtime-v1.md)。

## 贡献与许可

欢迎通过 [Issues](https://github.com/ZephyrKeXiner/ahakey-runtime-rust/issues) / [Pull Requests](https://github.com/ZephyrKeXiner/ahakey-runtime-rust/pulls) 贡献设备支持、协议处理、测试和文档。

真机反馈请附 OS、Rust 版本、设备/固件型号、复现步骤和脱敏日志。提交改动时执行上述检查，区分自动测试与真机观察；保持中英文 README 同步。

[Apache License 2.0](LICENSE)。

## Runtime 独立改键

`keymap.apply` 可直接修改指定键位，不要求完整配置基线或亮度支持；复用设备 worker 和持久化操作。见 [改键 API 与 IPC 客户端](docs/keymap-api.md)。

完整职责划分与文件定位见 [代码目录说明](docs/code-layout.md)。
