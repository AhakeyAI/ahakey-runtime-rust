# Runtime 代码目录

代码按功能分组。此次重构只调整职责、文件位置和内部可见性，保留原有 WebSocket 消息、操作记录格式、CLI 参数与 Rust 公共导入路径。

## 功能定位

| 功能 | 目录 / 文件 | 负责什么 |
| --- | --- | --- |
| 程序启动 | `src/app/` | 解析参数、加载显式初始化配置、启动后台任务、处理退出信号 |
| 设备发现 | `src/protocol/ble.rs` | 查找已连接设备、扫描、匹配和注册 X1 |
| 设备所有权 | `src/runtime/worker.rs` | 唯一持有 Hardware 的循环，串行处理连接、读取和写入 |
| 状态与事件 | `src/runtime/state.rs`、`device.rs` | 快照、事件顺序、输入状态、连接意图、设备状态及基线失效 |
| 配置与改键 | `src/configuration/`、`src/runtime/configuration.rs` | 配置模型、校验、作用范围、读回模型与设备写入调度 |
| 图片处理 | `src/oled/` | 图片解码、RGB565 编码、哈希、资源存储和二进制文件接收 |
| 设备图片上传 | `src/runtime/oled.rs` | 查询、上传、进度、核对及有限重试 |
| 操作管理 | `src/operation.rs`、`src/runtime/operations.rs` | 请求身份、幂等、持久受理、设备预留、暂停与继续 |
| WebSocket | `src/ipc/` | 监听、握手、每连接状态、认证、订阅、文件传输确认和请求分发 |
| 硬件接口 | `src/ahakey/hardware.rs` | Runtime 与具体型号之间的 Hardware 接口 |
| X1 实现 | `src/ahakey/x1/` | GATT 会话、命令编码、通知、状态解码、配置读回和 OLED 协议 |
| 模拟设备 | `src/ahakey/mock.rs` | 无蓝牙的显式 X1 模拟器 |
| 输入观察 | `src/input/`、`native/macos/InputMonitor.swift` | HID 状态和原生 helper 进程管理 |
| 持久化 | `src/storage/` | 操作 journal、重启恢复和私有文件原子替换 |
| 进程发现 | `src/discovery.rs` | discovery 文件、私有目录与实例锁 |
| 诊断 | `src/debug.rs`、`scripts/diagnostics/` | 有界日志、手动真机观察工具 |
| 对外数据 | `src/wire.rs` | 设备和配置文档投影、方法列表；Operation 从 operation 模型导出 |

## 一次调用经过哪里

```text
上位机
  → ipc/session：接收消息、超时与事件发送
  → ipc/client：认证、订阅与连接所属文件接收状态
  → ipc/dispatch：将业务请求交给 Runtime
  → runtime/operations：校验、幂等、持久受理与设备预留
  → runtime/worker：串行执行设备任务
  → runtime/configuration 或 runtime/oled：执行具体业务
  → ahakey/Hardware → ahakey/x1：编码、BLE 交换与设备确认
```

设备通知经 X1 通知处理进入 Runtime 状态，再由订阅事件返回上位机。文件准备经 `oled/transfer`、`codec` 和 `resources`，在显式提交设备上传之前不触碰硬件。

## 子目录的职责

- `configuration/model.rs` 保存可写配置、动作校验和合并；`apply.rs` 管理完整写入组与 scope；`readback.rs` 表达实际读到的字段；`keymap.rs` 表达独立改键。
- `oled/model.rs` 保存目标、资源和设备元数据；`codec.rs` 处理图片；`integrity.rs` 计算哈希/CRC；`resources.rs` 处理磁盘资源；`transfer.rs` 管理连接所属文件块。
- `ipc/server.rs` 管理监听与握手；`session.rs` 只负责 socket 收发；`client.rs` 管理认证和订阅；`resources.rs` 处理资源 RPC；`protocol.rs` 定义严格请求信封与错误；`dispatch.rs` 分发设备业务。
- `ahakey/x1/connection.rs` 管理会话与交换；`adapter.rs` 实现 Hardware；`commands.rs` 编码命令；`transport.rs` 是读回与上传共用的内部命令接口；其余文件分别处理通知、状态、配置读回和图片协议。
- `storage/journal.rs` 管理记录与恢复；`atomic.rs` 是 discovery、journal 和图片资源共用的私有文件原子写入实现。

新增功能时，将参数和数据放入对应功能模型，将执行流程放入 Runtime 对应功能文件，将固件字节处理留在具体型号目录。WebSocket 收发循环不编排设备步骤，图片处理不自行建立蓝牙连接。

## Rust 路径兼容

原有导入继续可用，内部代码使用新的功能位置：

| 原有路径 | 实际位置 |
| --- | --- |
| `keymap::*` | `configuration::keymap::*` |
| `mutation::Mutation` | `operation::Mutation` |
| `wire::Operation` | `operation::Operation` |
| `ahakey::model_x1::ModelX1` | `ahakey::x1::ModelX1` |
| `protocol::x1::*` | `ahakey::x1::commands::*` |
| `ahakey::oled::State` / `Entry` | `oled::State` / `Entry` |
| `ahakey::state::X1State` | `ahakey::x1::state::X1State` |

兼容通过模块导出完成，没有增加第二套实现。

## 测试与辅助代码

`tests/runtime.rs` 汇总 `tests/runtime/` 内按配置、WebSocket、设备事件、OLED、改键和存储分组的集成测试。`support/` 集中放置客户端、启动辅助、图片样本生成器及共享测试设备。其余输入、发现、诊断、协议及配置读回测试保持各自独立。

`examples/` 是开发者调用示例；`scripts/` 是联调和诊断工具；`native/macos/` 保留原生输入 helper。它们与生产 Rust 模块分别维护。

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```
