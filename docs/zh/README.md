<div align="center">

<h1>⌨️ AhaKey Runtime</h1>
<h4>AhaKey 硬件的 Rust 后台：BLE 通信、设备状态与 Studio IPC。</h4>

🌐 [English](../../README.md) | 🇨🇳 简体中文

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

已在 macOS / Rust 1.98.1 验证编译、自动测试和 **Studio dev 原有 Swift IPC 客户端联调**。已在一台 X1 真机验证发现、状态读取与 BLE 断开/重连；**配置写入和断电持久化尚未真机验证**；Windows/Linux 也未验证。OLED 资源、预览、Hook、语音、刷机和后台服务安装不在初版范围。

## 快速启动

使用支持 Rust 2024 edition 的工具链。目前以 macOS 开发为主，尚未声明最低 Rust 版本。

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

在 Xcode 的 **Edit Scheme → Run → Arguments → Environment Variables** 设置 `AHAKEY_RUNTIME_DISCOVERY` 为相同的绝对文件路径。Unix 下目录须为 `0700`，discovery 原子写入为 `0600`；日志不输出 token。Ctrl+C / SIGTERM 撤下 discovery，并给已受理操作最多 30 秒完成。

## 真机与首次初始化

停止旧版 Studio / Agent 对蓝牙的占用，然后运行：

```sh
cargo run --locked
```

开启蓝牙，允许承载进程的终端访问蓝牙。日志会打印稳定设备 ID：`x1-<平台外设 ID>`。可以通过 `--device-id` 固定选择；未发现设备时后台重试，IPC 仍可连接。

X1 无法完整回读键位和灯效。因此真实设备初次返回 `configuration: null`，Studio 禁止保存。建立可写基线需要显式提供**四个模式及全局亮度的完整配置**，并指定设备：

```sh
cargo run --locked -- --device-id 'x1-替换为发现的ID' --initialize /绝对路径/configuration.json
```

该命令会覆盖所选设备的键位/灯效配置，逐条等待 ACK，包括最终保存，并回读验证亮度。首次成功连接后执行一次。文件是包含 `device` / `modes` 的 Configuration 对象，不是 configuration.get 的结果或 RPC 信封。

可先从协议样例提取一份，再自行修改后执行初始化：

```sh
python3 - <<'PY'
import json
from pathlib import Path
source = json.loads(Path('contracts/runtime-v1/fixtures/configuration.json').read_text())
Path('initial-configuration.json').write_text(json.dumps(source['configuration'], indent=2) + '\n')
PY
```

基线仅在连续的设备会话中有效。断连、重启或发现外部亮度变化后失效，需显式初始化；后台不会自动把缓存写回设备。保存 ACK 不等于已验证断电持久化。

## 开发

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 scripts/check-studio-client.py --studio '../AhaKey Studio'
```

最后一项需要 macOS、Swift 和本地 Studio dev 分支。脚本读取原始三个 Swift Runtime 文件，不切换或修改 Studio 工作区；编译后启动隔离的 Rust 模拟器，验证真实 WebSocket 客户端，再清理进程和临时文件。

| 文件 | 职责 |
| --- | --- |
| `src/main.rs` | 启动参数、进程生命周期和信号 |
| `src/ipc.rs` / `wire.rs` | 认证、JSON-RPC、订阅、前端状态投影 |
| `src/configuration.rs` | 业务类型、严格校验、按 scope 合并 |
| `src/runtime.rs` | 每设备 worker、受理、状态、操作生命周期 |
| `src/storage.rs` / `discovery.rs` | 持久操作日志、崩溃恢复、私有发现文件和实例锁 |
| `src/ahakey/device.rs` / `hardware.rs` | AhaKey 容器及型号无关 Hardware 接口 |
| `src/ahakey/model_x1.rs` / `mock.rs` | X1 真机结构体与显式模拟器 |
| `src/ahakey/state.rs` | 公共状态及 X1 状态解码 |
| `src/protocol/ble.rs` / `x1.rs` | 设备发现、型号选择、X1 命令编码 |
| `contracts/runtime-v1` | Studio dev 原样协议 Schema 与样例 |

新型号新增自己的结构体，实现 `Hardware`，提供匹配规则、配置编码、状态解析和确认条件。IPC 和调度器不编码 X1 字节。详细约束与真机验收见 [初版架构](../runtime-v1.md)。

初版日志有界保留 **128 个已受理 operationId**，不会静默删除旧结果；达到上限后新写入返回 `BUSY`。归档/重置前先停服务、核对客户端未决操作。日志位于 discovery 同目录，真机使用 `discovery.state.json`，模拟器使用 `discovery.mock-state.json`。其中保留的配置是审计记录，不会直接冒充当前设备事实。

## 贡献与许可

欢迎通过 [Issues](https://github.com/ZephyrKeXiner/ahakey-runtime-rust/issues) / [Pull Requests](https://github.com/ZephyrKeXiner/ahakey-runtime-rust/pulls) 贡献设备支持、协议处理、测试和文档。

真机反馈请附 OS、Rust 版本、设备/固件型号、复现步骤和脱敏日志。提交改动时执行上述检查，区分自动测试与真机观察；保持中英文 README 同步。

[Apache License 2.0](../../LICENSE)。
