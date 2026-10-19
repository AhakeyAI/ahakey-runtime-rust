# Rust Runtime 初版架构

应用接入请先看 [WebSocket 应用开发参考](websocket-api.md)，其中汇总了当前全部接口及后续扩展。本文保留初版架构与行为说明。

对接基准：AhaKey Studio 本地 `dev`，提交 `5eb399c`。以其 `StudioFrontend/Runtime/RuntimeDTOs.swift`、`IPCRuntimeClient.swift`、`docs/rust-backend-integration.md` 与 `contracts/runtime-v1` 为准。更早的宽范围 runtime-studio 设计不是本次实现的 wire 契约。

## 型号与调度边界

```text
Studio Frontend / 原生客户端
  → ipc：WebSocket、认证、JSON-RPC
  → Runtime：权威状态、操作记录、每设备写入权
  → 每设备一个 worker，独占 Box<dyn Hardware>
      → ModelX1：X1 连接、通知、命令确认、配置编码入口
          → protocol/x1：仅 X1 的固件命令
          → btleplug：BLE
      → MockX1：显式模拟，不参与 BLE 匹配
```

`AhaKey` 保存设备标识、名称和硬件 trait 对象；不是一个塞进所有型号字段的超大结构体。新型号应新增 `model_*.rs`，实现 Hardware 并扩展发现选择。命令计划由具体型号生成；worker 只按顺序执行并根据型号确认结果更新进度。

X1 保存 Adapter 强引用（macOS Peripheral 内部依赖弱引用）、Peripheral、命令特征与通知流。连接后找到服务 `7340` 下的 `7343` / `7344`，先建立通知流/订阅，再查询状态。每次命令等待上限 4 秒；失败后断开旧会话，避免迟到 ACK 被下一条同命令接收。连接上限 15 秒，断开上限 3 秒。

连接期间不轮询状态：worker 等待 BLE 通知，`0x9B` 模式通知更新工作模式，`0x00` 完整状态通知更新电量/拨杆/亮度；蓝牙断线或服务变化事件使会话失效。首次连接查询一次完整状态，亮度写入后仍进行一次读回确认。等待命令 ACK 时收到的状态变化保留到 worker 继续处理，不再丢弃。模式通知的单字节 sequence 可回绕，按 BLE 有序交付处理。断连后每 3 秒重试连接，但不自动恢复配置基线。旧固件若不发送模式通知，不会通过轮询补偿；需升级到支持 `0x9B` 的固件。电量等字段在设备发送完整状态通知时刷新。

当前前端只使用第一台设备，因此自动发现固定选定一台 X1。匹配沿用已有 X1 服务和名称前缀 `ahakey` / `vibe code`；未来不同型号需要更精确的型号识别。`--device-id` 选择精确平台 ID，初始化必须指定它。

## API

| 方法 | 行为 |
| --- | --- |
| `runtime.hello` | token 认证、协议 1.0、实例 ID、实际方法列表 |
| `runtime.subscribe` | `topics:["state"]` / `cursor:null`，原子注册并返回完整快照 |
| `configuration.get` | 当前设备基线或 `configuration:null` |
| `configuration.apply` | 校验、幂等检查、设备预留、持久化，返回 accepted 后异步执行 |
| `operation.get` | 获取已受理操作，供丢失响应或重连恢复 |
| `device.connect` / `device.disconnect` | 入队后立即返回；通过完整 device.changed 表示结果 |

一个 WebSocket 文本消息一个 JSON-RPC 2.0 对象；字符串 id，camelCase 字段，内部 type 标签动作。拒绝 batch、二进制业务消息以及修改请求未知字段；JSON-RPC 通知不产生响应或设备变更。读取响应保留扩展空间。

服务只监听 `127.0.0.1` 的动态端口。允许无 Origin 原生客户端；拒绝携带 Origin 的浏览器握手。不按客户端自报 kind 提升权限。实例随机 token 不进入 URL 或日志。默认路径与 Studio 一致，可用 CLI 或环境变量覆盖。

订阅与快照共享状态锁，订阅响应先发送，再发边界序号之后的事件。全局 sequence 为 u64，wire 为十进制字符串。每个连接一个发送出口，不持状态锁等待网络或 BLE；慢客户端事件落后时关闭连接，前端重新获取快照。重启生成新的 instanceId。

## 配置及证据

共享配置校验：全局亮度 1–100；模式 0–3，不重复；scope 与 changes 模式集合一致；每模式恰好四个不同角色和九个不同状态映射。description 是最多 20 字节 ASCII；非法内容直接拒绝。快捷键仅允许 control/shift/alt/gui 和 0–255 HID usage，保留纯修饰键组合。宏 1–49 步，delayMs 为 0–765 且可被 3 整除，无参步骤不接受多余字段。

X1 写入：

1. 写亮度并回读相同值，不能只看旧固件可能返回的成功 ACK。
2. 写宏前清空快捷键层；写快捷键或 disabled 前清空宏层，再写动作和描述。
3. 每模式写九种状态灯效。固件灯效编号来自 LegacyDraftEncoding，不能使用菜单顺序编号。
4. 最后 `0x04` 保存，收到成功 ACK 才能完成。

全局亮度与所选模式覆盖，其他模式保留。正常受理要求可信完整基线；首次真实设备未知时，只能通过 `--initialize FILE --device-id ID` 显式初始化全部四个模式。断连、进程重启、观测到与已确认配置不同的亮度使基线失效。缓存仅保留审计；无法发现固件未暴露的外部键位/灯效修改，因此联调期间需保持 Runtime 为唯一设备 owner。

completed 表示每个命令达到型号定义的确认条件（X1 ACK + 亮度回读），不是额外宣称已做全配置读回或断电持久性验证。界面无需知道 Flash 布局、BLE MTU 或固件命令编号。

## 操作、失败与持久化

受理在同一状态锁中完成幂等查重、基线/就绪检查、设备写入权预留和持久化，之后入队。JSON 对象键顺序不影响幂等；模式、键、灯效、修饰键列表归一化。宏步骤顺序保持不变。相同 ID 与相同内容返回原操作；不同内容返回 OPERATION_ID_CONFLICT。原请求重复查询先于过期基线和繁忙检查。

worker 开始写入前再次检查就绪和 baseToken，防止受理期间收到的状态或断线通知使旧基线/会话失效。配置、连接、断开、状态通知使用同一 worker；繁忙时其他修改返回 BUSY。Studio 断开不取消已受理操作。

原子 JSON 日志记录请求和操作，文件 fsync、同目录 rename、目录 fsync 后才确认落盘。受理和终态均持久化；逐条 ACK 进度在内存广播。Runtime 在非 BLE Tokio 任务中，通过 spawn_blocking 完成磁盘写入；状态锁跨该持久化等待，网络与 BLE 不在锁中执行。磁盘错误使后续写入停止，避免没有恢复记录的执行。

重启时 accepted/running 转为 failed / effect=unknown / RUNTIME_RESTARTED，不自动重发。命令失败可能已经产生影响，即使第一条无 ACK 也不能声称 effect=none；初版保守标 unknown，保留已确认进度，清空基线。终态完成先发布 configuration.changed，再发布 operation.changed(completed)。

默认 discovery 同目录：

- `discovery.lock`：同路径实例锁，直到硬件退出阶段结束才释放。
- `discovery.state.json`：真机操作及已确认配置审计。
- `discovery.mock-state.json`：隔离的模拟器日志。

128 个 operationId 保留至显式维护；满后返回 BUSY，不自动淘汰幂等记录。备份/重置前停服务并核对前端未决操作，不可在运行中删除锁文件。不同 discovery 路径之间不共享实例锁；不要用多个路径同时运行真实硬件 owner。

SIGINT/SIGTERM 停止接受 IPC、移除 discovery、停止发现，给已受理写入 30 秒完成；超时退出留下日志，由下一次启动按未知结果恢复。

## 限制与错误

单消息 1 MiB、最多 16 个客户端、握手及认证 10 秒、发送 5 秒；每设备一个在途操作、容量 1 的 worker 队列、256 条全局事件缓存、128 条持久操作记录。不实现游标重放、通用取消接口、预览、Hook、语音、刷机、自动安装或系统启动项。OLED 文件接收、设备上传和专用暂停/继续已通过扩展提供，见下文。

稳定错误码包括 INVALID_PARAMS、INCOMPLETE_WRITE_GROUP、PERMISSION_DENIED、PROTOCOL_MISMATCH、DEVICE_NOT_FOUND、DEVICE_NOT_READY、BUSY、BASE_CONFLICT、UNSUPPORTED_CAPABILITY、OPERATION_ID_CONFLICT、OPERATION_NOT_FOUND、DEVICE_TIMEOUT、DEVICE_DISCONNECTED、DEVICE_REJECTED、STORAGE_ERROR、RUNTIME_RESTARTED。受理前返回 JSON-RPC error；受理后通过 Operation 保存硬件失败。运行中 storage 错误不自动重试硬件写入。

## 验证

`cargo test --locked` 覆盖 Studio fixtures、固件编码、无效参数、旧状态帧、私有 discovery / 实例锁、实际 WebSocket、双客户端事件顺序、断开后的操作完成、幂等与并发、部分写入失败、显式初始化、重启和损坏日志。

`scripts/check-studio-client.py` 直接从 Studio dev 读取并编译三个原始 Swift Runtime 文件，使用 Rust 模拟器验证七个方法与全部动作形状。该测试不修改 Studio 工作区，也不会启动真实 BLE。

真机仍需验收：发现/权限与连接、7343/7344 通道、不同固件状态帧、亮度回读与旧固件拒绝、真实快捷键/宏/禁用效果、九种状态灯效、保存 ACK、写入中断连与重新初始化、设备断电重连后的真实持久化。不能以模拟器或 UI 成功代替这些观测。

## macOS 重连问题修复（2026-09-22）

症状：首次连接正常，但断开后持续 `DEVICE_NOT_READY`。分阶段日志确认底层 connect 返回 `Peripheral no longer available`。btleplug 的 CoreBluetooth 后端在断开回调中移除内部外设记录；保留 Rust Peripheral clone 并不能保留该注册。

修复：ModelX1 每次在 macOS 建立新会话前，通过 `retrieve_peripherals(identifiers: [原 ID])` 重新取得并注册外设，再执行 connect、服务发现和通知订阅。设备 ID 不变，断连后的配置基线仍按原规则失效。连接各阶段保留底层错误日志，wire 错误码维持兼容。

真实设备已复现原故障，并验证修复后的状态读回和重复断开/重连。回归命令（需要正在运行的 Runtime、macOS Swift 及 Studio dev）：

```sh
python3 scripts/diagnostics/check-device.py 'x1-DEVICE_ID' --reconnect
```

省略 `--reconnect` 仅检查当前状态。添加该参数会主动断开/重连所选设备并使其配置基线失效；脚本不写配置。探针使用 Studio 原始 IPC 客户端，等待真实 ready 事件，失败退出码为 1。本次证据只覆盖发现、连接、状态与重连，不代表完整硬件配置验收。

## 独立改键扩展

新增 `keymap.apply`，不受完整配置基线和亮度能力限制；共享现有 worker、操作日志和 ACK 确认。见 [改键 API](keymap-api.md)。

## 基于远端固件 1.4.8 的自动读回

协议依据是 [eternal-dev 的 bc4f6e4](https://github.com/AhakeyAI/AhaKey-X1-hardware-source/tree/bc4f6e40dc11b486af9cedba528d24844e6225a9)。`Hardware::read_configuration` 返回设备可观察的配置字段；X1 通过 `9F` 确认 bit 10，再通过 `9D` 读取已有资源 `00` 绑定、`01` 灯效、`02` 全局灯光。没有增加固件命令、资源或能力位。

每次采集为 217 个资源请求。固件没有配置 revision，Runtime 在同一 worker 中保持写入串行，连续两次采集相同后发布；最多采集三次，总上限 45 秒。这是两次采集的一致性检查，不是设备原子快照；Runtime 应为唯一配置 owner。读取期间设备保持 connecting，完成后刷新状态、baseToken，发布 configuration.changed 并进入 ready。

`configuration.get` 新增可选 readback，包含四模式键位、灯效、亮度和任务灯光模式。描述未被固件暴露，保持 null；当前 Action 无法表达的绑定 action 为 null，rawBinding 保留全部 100 个原始字节。完整 configuration 不由缺失字段填充生成，因此仍为 null。旧客户端可忽略 readback，完整配置保存的基线要求不变。

断连、外部亮度变化和成功写入会清除旧 readback；重连重新读取。读取不写硬件、不创建持久 Operation；失败丢弃本次采集。旧固件通用 ACK 识别为不支持；能力查询超时或拒绝时重建一次不探测的会话，使基本设备功能继续可用。

测试样本 `tests/fixtures/x1-readback-1.4.8.json` 来自固定远端提交中原样的 C 查询分支，在主机上编译执行，使用合成 RAM 数据生成 218 个能力/分块响应；它不是实物抓包。

## CP2 OLED 上传扩展

当前增加 resource.begin/finish/abort、OLED 元数据与图片上传，以及 OLED 专用暂停/继续。文件经 WebSocket 二进制块传至本地资源库；图片解码在后台线程，设备操作复用现有唯一 worker 与持久 journal。协议、能力识别、资源范围、暂停恢复及验证边界见 [OLED API](oled-api.md)。OLED 受理中/执行中的操作在进程重启后变为 paused/unknown，保留资源供用户主动继续，不会自动写入。
