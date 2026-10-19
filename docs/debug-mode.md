# 新固件联调：Runtime 调试模式

Debug 构建默认开启，Release 构建默认关闭（由编译时 `debug_assertions` 决定）。开启只增加诊断日志，不改变协议校验，不增加固件探测命令，也不主动修改
键位或持久配置。Frontend 原有的保存操作仍会照常写入设备。

## 启动

先退出现有 Studio/Runtime，避免同一 discovery 路径的所有权锁冲突。在 Rust 仓库运行：

```sh
cargo run --locked
```

无需 Studio，进程会持续运行并输出诊断，直到 Ctrl-C 停止。
Release 构建需要日志时使用 `cargo run --release --locked -- --debug`。
临时关闭 Debug 构建的诊断：`AHAKEY_DEBUG=0 cargo run --locked`。
`AHAKEY_DEBUG=1` 强制开启；未设置或其他值沿用构建默认值。
显式 `--debug`、`--debug-log` 或 `AHAKEY_DEBUG_LOG` 仍会启用日志，即使设置了 `AHAKEY_DEBUG=0`。

默认自动发现 AhaKey。需要指定设备时添加 `--device-id x1-设备UUID`。
终端会混合显示普通错误和 JSON 诊断行；保存纯 JSONL 日志使用：

```sh
cargo run --locked -- --debug-log "/tmp/ahakey-debug-$(date +%Y%m%d-%H%M%S).jsonl"
```

`--debug-log` 自动启用调试，诊断输出到文件，普通错误仍在终端。
文件必须不存在，父目录必须已存在；文件已存在或不可写会在连接设备前报错。
Unix 上新文件权限为 `0600`。日志不自动轮转，建议一次联调使用一个新文件。

启动这个外部 Runtime 后再打开 Studio，它可以通过默认 discovery 连接该进程。
无需修改 Frontend 协议。Studio 退出后，此手动启动的 Runtime 用 Ctrl-C 停止。

也支持启动环境变量，适合从 IDE 启动已经重新打包了此版本 Runtime 的应用：

```text
AHAKEY_DEBUG=1
AHAKEY_DEBUG_LOG=/tmp/ahakey-debug-new-session.jsonl
```

文件路径变量本身也启用调试；命令行 `--debug-log` 优先于环境变量中的路径。
环境变量必须传给实际 Runtime 进程；已在运行的 Runtime 不会因 Frontend 改变环境
而切换调试状态。单独的诊断 example 不会自动初始化这个日志功能。

无需硬件检查模式：

```sh
cargo run --locked -- --mock
```

## 如何读日志

每行包含 `event`、Unix 毫秒时间字符串 `unixMs`、进程内单调时间 `elapsedMs` 和 `data`。

| event | 含义 |
| --- | --- |
| `runtime.start` / `runtime.stop` | 版本、是否模拟、进程开始及正常停止 |
| `ble.discovery` / `ble.notFound` | 扫描开始及没有匹配设备 |
| `ble.connect` / `ble.connectResult` | 连接尝试及结果 |
| `ble.services` | 实际发现的服务、特征 UUID 与属性 |
| `ble.tx` | 准备发送的原始命令；不代表 GATT 写入或固件执行成功 |
| `ble.rx` | 收到的通知，包含原始 hex、UUID、长度、帧格式是否有效 |
| `ble.interleaved` | 等待某条命令时收到其他 opcode；可能是正常状态推送 |
| `ble.invalidStatus` | opcode 为状态但内容不能被当前状态解析器接受 |
| `ble.exchange` | 命令耗时、失败阶段及错误；`error:null` 表示现有协议确认成功 |
| `device.state` / `device.changed` | 解析出的模式、电量、拨杆等状态及对外状态变化 |
| `operation.command` / `operation.changed` | 配置操作的命令进度与最终结果 |
| `configuration.invalidated` | 已知配置基线失效 |
| `ipc.request` | 已识别的方法、处理耗时及错误，不记录请求内容 |
| `ble.linkLost` / `ble.disconnect` | 观察到连接失效或主动断开 |

例如新固件推送了 Runtime 不认识的 opcode，它仍会出现在 `ble.rx`，不会自动转成
业务事件或当成状态。无效帧也先记录，再按原协议规则过滤。
`ble.exchange` 的 `phase` 可区分 GATT 写入失败与等待固件响应超时；耗时是电脑侧
一次交换用时，不等于物理按键延迟。日志不改变当前 ACK 和状态确认标准。

原始帧最多保留前 1024 字节，完整长度与 `truncated` 会说明是否截断。
后台线程写日志，设备处理线程使用容量 256 的非阻塞队列；日志过载时丢弃诊断，
正常退出时用 `debug.dropped` 报告数量。强制终止可能丢失末尾日志，不能把日志缺失
当作设备未执行命令的证据。

不记录 IPC token、完整请求/响应、客户端名称或 HID 监控按键事件。BLE 原始报文
仍可能含键位配置、描述或新固件自定义数据，因此分享日志前应核对这些内容。

## 建议的联调步骤

1. 启动调试 Runtime，确认服务 `7340`、命令特征 `7343` 与通知特征 `7344`。
2. 确认初始 `ble.tx` 的状态查询得到 `ble.rx`，随后产生 `device.state`。
3. 切换设备模式、拨杆，对照原始通知和解析结果；新字段仍能从 hex 中看到。
4. 通过 Studio 执行需要验证的操作，用 operationId 串联命令进度和结果。
5. 测试断连和重连，最后 Ctrl-C 停止并保留日志。

调试模式不提供任意原始命令发送接口；没有发起写操作时不会借联调之名初始化设备。
