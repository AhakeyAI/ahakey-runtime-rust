# AhaKey Runtime WebSocket 应用开发参考

适用于通过本机 Runtime 开发桌面应用、CLI 或桌面 WebView 应用的开发者。当前协议为 **1.0**，Runtime 包版本为 **0.1.0**。应用通过 WebSocket 管理连接、订阅状态、写入配置及查询操作结果；BLE 连接与硬件命令由 Runtime 统一处理。

本参考覆盖当前实现的全部 15 个 RPC 方法和 7 类状态事件，包括初版契约之后增加的 `keymap.apply`、输入监听和 [OLED 上传扩展](oled-api.md)。另有连接内文件传输确认通知。示例中的设备 ID、token、baseToken、instanceId 和 subscriptionId 均为占位值，实际值必须从当前连接取得。

## 目录

- [快速接入](#快速接入)
- [发现与连接](#发现与连接)
- [消息约定](#消息约定)
- [接口总览](#接口总览)
- [认证与订阅](#认证与订阅)
- [设备连接控制](#设备连接控制)
- [读取与写入配置](#读取与写入配置)
- [独立改键](#独立改键)
- [操作查询与恢复](#操作查询与恢复)
- [事件参考](#事件参考)
- [数据模型](#数据模型)
- [错误处理](#错误处理)
- [限制与兼容性](#限制与兼容性)
- [可运行的只读客户端](#可运行的只读客户端)
- [实现与相关资料](#实现与相关资料)

## 快速接入

1. 读取本机 `discovery.json`，取得 `endpoint` 和 `token`。
2. 使用原生 WebSocket 客户端连接 `endpoint`，在升级完成后 10 秒内调用 `runtime.hello`。
3. 保存返回的 `instanceId`，检查 `methods` 是否包含应用需要的方法。
4. 调用 `runtime.subscribe`，用完整快照建立本地状态，再依序消费 `runtime.event`。
5. 从 `devices` 选择目标设备，等待其 `connectionState` 为 `ready`。
6. 写入前调用 `configuration.get` 取得最新 `baseToken`。仅改键可使用 `keymap.apply`；完整配置保存还要求非空配置基线。
7. 为一次写入生成并保留 `operationId`，调用写入方法后，通过事件或 `operation.get` 等待终态。
8. 连接中断后重新读取 discovery、认证和订阅，再查询原 `operationId`，核对结果后决定下一步。

应用应由一个连接管理器统一接收消息，按 `id` 分发 RPC 响应，按 `method: "runtime.event"` 分发事件。事件和响应可以交错到达，不能把“下一条消息”直接当成某次调用的响应。

## 发现与连接

### Discovery 文件

| 平台 | 默认路径 |
| --- | --- |
| macOS | `~/Library/Application Support/AhaKey/runtime/discovery.json` |
| Windows | `%LOCALAPPDATA%/AhaKey/runtime/discovery.json` |
| Linux 等其他平台 | `$XDG_RUNTIME_DIR/ahakey/discovery.json`；未设置时为 `~/.local/state/ahakey/discovery.json` |

`~` 需要由客户端展开为用户目录。Runtime 的 `--discovery ABSOLUTE_PATH` 优先于 `AHAKEY_RUNTIME_DISCOVERY` 环境变量，再其次为平台默认路径。应用和 Runtime 必须使用同一个文件路径。

```json
{
  "endpoint": "ws://127.0.0.1:54321/",
  "token": "TOKEN_FROM_DISCOVERY"
}
```

端口由操作系统动态分配，不要硬编码。当前文件仅包含上述两个字段，`instanceId` 通过认证响应获取。Runtime 正常退出会删除 discovery；进程异常退出可能留下旧文件。读取或连接失败时重新读取文件并退避重试，不要删除实例锁或启动多个真实设备 owner。

Unix 上 discovery 父目录必须为私有目录（通常 `0700`），文件以 `0600` 写入。token 仅发送给本机 Runtime，不放入 URL、日志或遥测。客户端在发送 token 前应校验 endpoint 为预期的本机地址。

### WebSocket 升级要求

- 当前进程监听 `127.0.0.1`，使用 `ws://`，路径必须为 `/`，不能带 query。
- 握手只要包含 `Origin` 头就会被拒绝（HTTP 403），包括 `Origin: null`。
- 普通浏览器页面不能直接连接。Electron、Tauri 或 WebView 应通过原生宿主进程连接，再将业务数据传给界面。
- 不使用 HTTP Bearer 认证或 WebSocket 子协议承载 token；认证通过 `runtime.hello` 完成。

## 消息约定

每个 WebSocket **文本消息**承载一个 JSON-RPC 2.0 对象。请求必须带非空字符串 `id`，最长 128 个 UTF-8 字节。同一连接内尚未完成的请求应使用不同 ID。`id` 用于匹配 RPC 响应，写入参数中的 `operationId` 用于持久化追踪和幂等，两者不能混用。

```json
{
  "jsonrpc": "2.0",
  "id": "request-1",
  "method": "configuration.get",
  "params": {"deviceId": "x1-DEVICE_ID"}
}
```

成功响应使用 `result`：

```json
{
  "jsonrpc": "2.0",
  "id": "request-1",
  "result": {
    "deviceId": "x1-DEVICE_ID",
    "baseToken": "BASE_TOKEN",
    "configuration": null
  }
}
```

失败响应使用 `error`，应用应根据 `error.data.code` 分支处理：

```json
{
  "jsonrpc": "2.0",
  "id": "request-1",
  "error": {
    "code": -32000,
    "message": "DEVICE_NOT_FOUND",
    "data": {"code": "DEVICE_NOT_FOUND"}
  }
}
```

不支持 batch 数组或无 `id` 的变更调用。二进制消息仅用于 [resource 文件接收](oled-api.md#二进制块)，不能承载任意硬件命令。格式有效的 JSON-RPC 通知会被忽略，不产生响应或修改。请求信封和参数不接受未知字段；响应和快照的读取应容忍未来新增字段及未知事件类型。

## 接口总览

| 方法 | 参数要点 | 成功结果 | 完成含义 |
| --- | --- | --- | --- |
| `runtime.hello` | `protocol`, `client`, `token` | 协议、实例、版本、方法列表 | 当前连接认证成功 |
| `runtime.subscribe` | `topics: ["state"]`, `cursor: null` | `subscriptionId`, `snapshot` | 快照及后续事件的原子订阅已建立 |
| `device.connect` | `deviceId` | `{}` | 连接任务已入队 |
| `device.disconnect` | `deviceId` | `{}` | 断开任务已入队 |
| `configuration.get` | `deviceId` | 配置文档 | 返回 Runtime 当前已知基线 |
| `configuration.apply` | 操作 ID、设备、基线、范围、配置 | `operationId`, `status` | 操作已受理或返回同 ID 的已有状态 |
| `keymap.apply` | 操作 ID、设备、基线、模式、键列表 | `operationId`, `status` | 操作已受理或返回同 ID 的已有状态 |
| `operation.get` | `operationId` | 完整 Operation | 返回该操作当前状态 |
| `resource.begin` / `finish` / `abort` | 文件长度与哈希 / uploadId | 接收任务 / 本地资源 / `{}` | 详见 [OLED API](oled-api.md) |
| `oled.get` / `upload` | deviceId / 资源及目标 | 元数据 / 操作 ID | 详见 [OLED API](oled-api.md) |
| `operation.pause` / `resume` | operationId | 完整 Operation | OLED 专用暂停与显式继续 |

除了 `runtime.hello`，所有方法都要求先完成认证。当前 hello 返回的 `methods` 列表包含另外 14 个方法，**不包含 `runtime.hello` 自身**。

## 认证与订阅

### runtime.hello

每次建立新 WebSocket 都要重新认证。`client.name`、`version`、`kind` 均为必填非空字符串；`kind` 只是客户端标识，不授予额外权限。

```json
{
  "jsonrpc": "2.0",
  "id": "hello-1",
  "method": "runtime.hello",
  "params": {
    "protocol": {"major": 1, "minor": 0},
    "client": {"name": "My AhaKey App", "version": "1.0.0", "kind": "desktop"},
    "token": "TOKEN_FROM_DISCOVERY"
  }
}
```

```json
{
  "jsonrpc": "2.0",
  "id": "hello-1",
  "result": {
    "protocol": {"major": 1, "minor": 0},
    "instanceId": "RUNTIME_INSTANCE_ID",
    "runtimeVersion": "0.1.0",
    "methods": [
      "runtime.subscribe", "configuration.get", "configuration.apply", "keymap.apply",
      "operation.get", "device.connect", "device.disconnect"
    ]
  }
}
```

当前仅检查协议 major 为 `1`，minor 不作为拒绝条件，服务端报告支持的 `1.0`。客户端应核对返回协议并检查所需方法，不能只依据包版本假设能力。

### runtime.subscribe

```json
{
  "jsonrpc": "2.0",
  "id": "subscribe-1",
  "method": "runtime.subscribe",
  "params": {"topics": ["state"], "cursor": null}
}
```

```json
{
  "jsonrpc": "2.0",
  "id": "subscribe-1",
  "result": {
    "subscriptionId": "SUBSCRIPTION_ID",
    "snapshot": {
      "instanceId": "RUNTIME_INSTANCE_ID",
      "sequence": "12",
      "devices": [{
        "deviceId": "x1-DEVICE_ID",
        "name": "AhaKey X1",
        "connectionState": "ready",
        "batteryPercent": 82,
        "workMode": 0,
        "lever": "manual"
      }],
      "operations": [],
      "input": {"status": "disabled", "permission": "unknown", "devices": []}
    }
  }
}
```

只接受 `topics` 恰好为 `["state"]`，`cursor` 为 `null` 或省略。不支持历史游标重放、其他 topic 或单独的取消订阅方法。同一连接再次订阅会替换旧订阅；关闭连接结束订阅。

快照和事件订阅共享同一个状态边界，订阅响应先于该订阅的事件发送。客户端先替换本地快照，再处理大于快照 `sequence` 的后续事件。快照包含设备、保留的操作和输入状态，**不包含设备配置文档**，配置需通过 `configuration.get` 单独获取。

`sequence` 是 u64 的十进制字符串，JavaScript 比较时使用 `BigInt`，不要转成 `Number`。序号属于整个 Runtime 实例，不属于单个设备。实例重启后 `instanceId` 改变，旧序号作废；重连一律重新订阅取快照。

## 设备连接控制

### device.connect 和 device.disconnect

```json
{
  "jsonrpc": "2.0",
  "id": "connect-1",
  "method": "device.connect",
  "params": {"deviceId": "x1-DEVICE_ID"}
}
```

```json
{"jsonrpc": "2.0", "id": "connect-1", "result": {}}
```

断开使用相同参数，将方法名改为 `device.disconnect`。成功响应仅表示控制任务入队，实际结果通过 `device.changed` 或重新订阅后的快照确认。这两种控制任务没有 `operationId`，不能通过 `operation.get` 查询。

设备发现后默认尝试连接；意外断线后约每 3 秒重试。显式 `device.disconnect` 会关闭该设备的自动重连意图，直到调用 `device.connect`。连接失败可能经历反复的 `connecting` / `disconnected`，没有单独的连接失败 Operation。设备写入或控制任务占用 worker 时，其他变更可能返回 `BUSY`。

`device.connect` 的设备 ID 必须来自 Runtime 已登记设备列表；它不是按任意蓝牙地址添加设备的接口。当前进程自动选择一台 X1，可通过启动参数 `--device-id` 指定。

## 读取与写入配置

### configuration.get

请求及返回形状见[消息约定](#消息约定)。它返回 `deviceId`、不透明字符串 `baseToken` 和 `configuration`。设备断开时仍可能返回文档；写入则要求设备 `ready`。设备已有配置或 readback 且 worker 正忙时，读取可能返回 `BUSY`。

此方法**不会读取键盘 Flash**。真实设备初连、Runtime 重启、设备断连或观测到外部亮度变化后，完整配置可能为 `null`。`null` 表示完整配置未知，界面应禁用完整配置保存，避免将本地默认值误当作设备内容。

Runtime 以 [Fireware-harness `076382b`](https://github.com/AhakeyAI/AhaKey-X1-hardware-source/tree/076382b2214289ac7bc1735d669d7de78730c4da) 为基准，用 `0x87` 读取按键及描述、`0x88` 读取亮度和灯效。在首次连接、重连及进程重启后自动读取，连续两次采集一致后，在可选字段 `readback` 中提供。读取期间设备保持 `connecting`，完成后更新 baseToken、发布 `configuration.changed`，再进入 `ready`。

`readback` 与完整 `configuration` 分开：所有现有可写字段完整可读且可表示时，自动建立完整 `configuration`。不能表达的动作或非 ASCII 描述为 `null`，原始字节保留。固件没有配置 revision，Runtime 使用连续采集结果比较，不用未知字段生成完整基线。

`baseToken` 用于防止旧会话或并发修改覆盖当前状态。不要解析、递增或自行生成。成功写入和基线失效后 token 会改变。收到配置事件或写入终态后，应重新读取再开始下一次编辑/保存。

### configuration.get 的 readback 扩展

没有完成读回、固件不支持、断连、基线失效或成功写入后，`readback` 字段省略。新客户端应兼容缺失；旧客户端可忽略新增字段。`readback` 不进入持久操作日志。

| 字段路径 | 内容 |
| --- | --- |
| `readback.device.brightnessPercent` | 设备当前亮度 `0..100`；SDK 可设置为 0，完整配置写入仍要求 `1..100` |
| `readback.device.taskLightMode` | `null`；当前固件的 `0x98` 为只设置接口，不能查询 |
| `readback.modes` | 四个模式，模式编号 `0..3` |
| `readback.modes[].keys` | 每模式四个角色 |
| `readback.modes[].keys[].role` | `voice`, `approve`, `reject`, `submit` |
| `readback.modes[].keys[].action` | 已解码 Action；现有 Action 无法表达时为 `null`，不是 disabled |
| `readback.modes[].keys[].description` | 固件读回的 ASCII 描述；非 ASCII 或读取失败时为 `null` |
| `readback.modes[].keys[].rawBinding` | 读取成功时补零至长度 100：第 0 字节 subtype，第 1 字节为 payload 长度；读取失败时为 `[]` |
| `readback.modes[].keys[].rawDescription` | 成功时返回去掉尾部空格/零字节的原始描述字节；读取失败时省略 |
| `readback.modes[].keys[].readStatus` | 固件状态：`0` 成功、`2` 越界/不支持的保存类型、`3` 数据超过单帧上限；2/3 不包含地址回显 |
| `readback.modes[].lights` | 每模式九种 `{state,effect}` 映射，名称与 Configuration 相同 |

键索引与角色对应关系为 `0=voice`、`1=approve`、`2=reject`、`3=submit`。raw-F18 固件的物理 Voice Key 固定输出 F18，保存的绑定不等于该键的实际输出行为。

readback 是连接期间读取到的设备内容。成功写入后清除旧 readback，界面使用本次 Operation 结果确认写入；下一次连接会重新读回。应用不能把 readback 原样作为 configuration.apply 的 changes，也不能将未知描述显示为设备上的空描述。

### configuration.apply

要求设备就绪、支持当前完整配置写入，并且 Runtime 已有可信完整配置基线。以下示例覆盖模式 `0` 的全部四个键和九种灯效，同时设置全局亮度；其他模式保留。

```json
{
  "jsonrpc": "2.0",
  "id": "apply-1",
  "method": "configuration.apply",
  "params": {
    "operationId": "OPERATION_CONFIG_1",
    "deviceId": "x1-DEVICE_ID",
    "baseToken": "BASE_TOKEN",
    "scope": {"deviceFields": ["brightnessPercent"], "modes": [0]},
    "changes": {
      "device": {"brightnessPercent": 60},
      "modes": [{
        "mode": 0,
        "keys": [
          {"role": "voice", "action": {"type": "shortcut", "modifiers": [], "keyCode": 7}, "description": "D"},
          {"role": "approve", "action": {"type": "shortcut", "modifiers": [], "keyCode": 9}, "description": "F"},
          {"role": "reject", "action": {"type": "shortcut", "modifiers": [], "keyCode": 13}, "description": "J"},
          {"role": "submit", "action": {"type": "shortcut", "modifiers": [], "keyCode": 14}, "description": "K"}
        ],
        "lights": [
          {"state": "notification", "effect": "pulseCenter"},
          {"state": "permissionRequest", "effect": "approvalWait"},
          {"state": "postToolUse", "effect": "successSweep"},
          {"state": "preToolUse", "effect": "singleMove"},
          {"state": "sessionStart", "effect": "rainbowWave"},
          {"state": "stop", "effect": "middleLight"},
          {"state": "taskCompleted", "effect": "successSweep"},
          {"state": "userPromptSubmit", "effect": "breathing"},
          {"state": "sessionEnd", "effect": "off"}
        ]
      }]
    }
  }
}
```

```json
{
  "jsonrpc": "2.0",
  "id": "apply-1",
  "result": {"operationId": "OPERATION_CONFIG_1", "status": "accepted"}
}
```

参数规则：

- `operationId` 和 `baseToken` 为 1–128 UTF-8 字节的字符串；`deviceId` 为 1–256 字节。
- `scope.deviceFields` 当前必须恰好为 `["brightnessPercent"]`，所以每次完整配置保存都会包含全局亮度。
- `scope.modes` 无重复，且必须与 `changes.modes` 中的模式集合完全一致。
- `changes.modes` 包含 1–4 个不同模式，每个模式必须提交全部四个角色和全部九种状态灯效，不能只提交其中一个字段。
- `changes` 是 `Configuration`，不是 `ConfigurationDocument` 或 RPC 信封。字段范围见[数据模型](#数据模型)。

即使请求提交全部四个模式，**WebSocket 也不能绕过未知基线完成初始化**。读回不完整时，操作人员可显式运行 `--initialize FILE --device-id ID`，它会覆盖全部四模式和亮度，具体步骤见 [README](../README.md#真机与首次初始化)。应用只需改键时使用下节接口。

## 独立改键

### keymap.apply

此方法在完整配置为 `null` 时仍可使用，但仍要求当前 `baseToken` 和就绪设备。调用前检查 hello 的 `methods` 包含 `keymap.apply`，以兼容早期 Runtime。

```json
{
  "jsonrpc": "2.0",
  "id": "keymap-1",
  "method": "keymap.apply",
  "params": {
    "operationId": "OPERATION_KEYMAP_1",
    "deviceId": "x1-DEVICE_ID",
    "baseToken": "BASE_TOKEN",
    "mode": 2,
    "keys": [
      {"index": 0, "action": {"type": "shortcut", "modifiers": [], "keyCode": 7}},
      {"index": 1, "action": {"type": "shortcut", "modifiers": [], "keyCode": 9}},
      {"index": 2, "action": {"type": "shortcut", "modifiers": [], "keyCode": 13}},
      {"index": 3, "action": {"type": "shortcut", "modifiers": [], "keyCode": 14}}
    ]
  }
}
```

```json
{
  "jsonrpc": "2.0",
  "id": "keymap-1",
  "result": {"operationId": "OPERATION_KEYMAP_1", "status": "accepted"}
}
```

`mode` 为 `0..3`；`keys` 包含 1–4 个不重复索引，`index` 为 `0..3`。支持同样的 `shortcut` / `macro` / `disabled` 动作，ID 和 token 长度限制与完整保存相同。

该操作仅替换指定键的动作并保存，不写入描述、灯效或亮度。未列出的键及其他模式保留。这里的索引是固件位置，不是 HID usage；与完整配置角色的对应关系为 `0=voice`、`1=approve`、`2=reject`、`3=submit`。示例是 UI 模式 3 的 DFJK。

写入成功会更新 baseToken。已有完整基线时更新相应动作；原本未知时仍保持 `configuration: null`。应用应显示“键位保存成功”，避免将其显示为完整配置保存成功。它是持久改键接口，没有临时接管、旧绑定备份或自动恢复功能。

## 操作查询与恢复

### operation.get

```json
{
  "jsonrpc": "2.0",
  "id": "operation-1",
  "method": "operation.get",
  "params": {"operationId": "OPERATION_KEYMAP_1"}
}
```

```json
{
  "jsonrpc": "2.0",
  "id": "operation-1",
  "result": {
    "operationId": "OPERATION_KEYMAP_1",
    "deviceId": "x1-DEVICE_ID",
    "status": "completed",
    "progress": 1.0,
    "effect": "complete",
    "errorCode": null
  }
}
```

### 生命周期与完成条件

正常生命周期为 `accepted → running → completed`，失败可从 accepted 或 running 进入 `failed`。协议终态还保留 `cancelled`，当前没有通用取消方法，也不会主动产生该状态。OLED 另有 `paused`，保留任务供显式继续，不是终态。客户端应将 `completed`、`failed`、`cancelled` 都视为终态。

| 字段 | 含义 |
| --- | --- |
| `status` | 当前操作阶段，不能用 RPC 成功替代终态判断 |
| `progress` | `0..1`，按已确认命令数计算；不是剩余时间估计，达到 1 也应继续检查终态 |
| `effect: none` | 尚未执行硬件命令，例如执行前基线检查失败 |
| `effect: unknown` | 命令在途、失败或重启后，实际硬件影响无法确定 |
| `effect: partial` | 运行中已有命令获得确认 |
| `effect: complete` | 全部命令满足当前型号的确认条件 |
| `errorCode` | 正常为 `null`；失败时为业务错误码 |

首次受理先持久化，再返回 `accepted` 并异步执行；应用断开不会取消操作。X1 的 `completed` 表示所有写入命令和最终保存收到成功 ACK，完整配置写入还验证亮度回读；不等于完成所有配置读回或证明断电持久性。操作不是硬件事务，中途失败可能已经部分生效，不会自动回滚或重发。

### 幂等与断线恢复

为每次用户确认的写入生成全新 `operationId`（例如 UUID），在发请求前保留该 ID、目标设备和原始参数。相同 ID 与相同规范化请求返回原操作，不再写硬件；相同 ID 配不同内容返回 `OPERATION_ID_CONFLICT`。判重早于设备就绪、繁忙和基线检查，所以查重时必须保留原始 `baseToken`，不能换新 token 后重发旧 ID。

对象字段顺序、模式顺序、角色/灯效列表顺序、改键索引顺序及修饰键顺序不影响判重；宏步骤顺序影响请求内容。返回已有操作时，写入 RPC 的 `status` 可能已经是 `running`、`completed` 或 `failed`，不保证总为 `accepted`。

如果响应超时或 WebSocket 中断：

1. 将本次结果显示为“待确认”，不要立即生成新 ID 重发。
2. 重新读取 discovery、连接、认证、订阅，核对原 `operationId`，可从快照或 `operation.get` 获取。
3. 已存在则跟随该记录到终态。不存在时，确认连接的是同一个 Runtime 数据存储，再考虑以完全相同参数和相同 ID 重试。
4. 如果重试返回 `BASE_CONFLICT`，重新读取配置并让用户核对后，作为新操作提交；不要静默改 token 覆盖。

Runtime 使用同一 journal 重启后，未完成配置/改键恢复为 `failed` / `effect: unknown` / `errorCode: RUNTIME_RESTARTED`；未完成 OLED 恢复为 `paused` / unknown，已有暂停记录保留。不会自动重放。终态记录可继续查询；切换 discovery 路径、mock/真实模式或日志被维护重置后，不能假设旧 ID 仍存在。

## 事件参考

所有事件均为服务端 JSON-RPC 通知，外层没有 `id`：

```json
{
  "jsonrpc": "2.0",
  "method": "runtime.event",
  "params": {
    "subscriptionId": "SUBSCRIPTION_ID",
    "instanceId": "RUNTIME_INSTANCE_ID",
    "sequence": "13",
    "type": "device.changed",
    "data": {
      "deviceId": "x1-DEVICE_ID",
      "name": "AhaKey X1",
      "connectionState": "ready",
      "batteryPercent": 82,
      "workMode": 1,
      "lever": "manual"
    }
  }
}
```

| `type` | `data` | 客户端处理 |
| --- | --- | --- |
| `device.changed` | 完整 Device | 按 `deviceId` 替换设备状态 |
| `operation.changed` | 完整 Operation | 按 `operationId` 替换操作状态，终态后更新保存 UI |
| `configuration.changed` | `{"deviceId":"…"}` | 标记旧文档过期，重新获取配置及 token |
| `configuration.invalidated` | `{"deviceId":"…"}` | 立即停止使用旧基线和 token，再获取文档 |
| `input.changed` | 完整 InputState | 替换输入状态，包括设备和按住列表 |
| `input.key` | KeyEvent | 按输入设备更新 held usages，按需显示观察事件 |
| `oled.changed` | deviceId、target、resourceId | 指定资源已提交并核验；全部元数据通过 oled.get 查询 |

配置/改键成功写入时先发布 `configuration.changed`，再发布终态 `operation.changed(completed)`。OLED 成功时对应发出 `oled.changed`。配置事件仅携带设备 ID，不含新 token；两个事件可能先于写入 RPC 响应被客户端读到。客户端应容忍事件提前创建操作记录，以及异步 `configuration.get` 返回 `BUSY`，在操作终态后重读。

客户端应核对当前 `subscriptionId` 和 `instanceId`，按事件顺序更新状态。遇到无法解释的序号缺口、实例变化或连接中断时，重新取完整快照。未知事件可以忽略业务内容，但仍应推进该连接的序号。

### 输入监听事件

输入监听来自 macOS 原生 helper，启动 Runtime 时指定 `--input-monitor`，并将 `ahakey-input-monitor` 放在 Runtime 可执行文件同目录。mock 模式不启动真实监听。没有输入监听仍可使用其他 RPC。

`snapshot.input` 与 `input.changed.data` 使用同一结构：

```json
{
  "status": "listening",
  "permission": "granted",
  "devices": [{
    "inputDeviceId": "hid-12345",
    "name": "AhaKey X1",
    "pressedUsages": [7]
  }]
}
```

| 字段 | 当前值 |
| --- | --- |
| `status` | `disabled`, `starting`, `permissionRequired`, `permissionDenied`, `deviceUnavailable`, `listening`, `error` |
| `permission` | `unknown`, `notDetermined`, `denied`, `granted` |
| `pressedUsages` | 当前已观察为按住的 HID usage 数组 |

`input.key.data` 示例为按下，松开时 `pressed` 为 `false`，`heldMs` 可为持续毫秒数：

```json
{
  "inputDeviceId": "hid-12345",
  "name": "AhaKey X1",
  "usage": 7,
  "pressed": true,
  "timestampMs": "123456",
  "heldMs": null
}
```

`timestampMs` 是 HID 单调时间的十进制字符串，不是 UTC；`heldMs` 是观察到的按下至松开间隔，可为小数。usage 范围为 `4..255`，包括修饰键。`inputDeviceId` 是本次 macOS HID 连接身份，**不能与 BLE deviceId 或物理键 index 直接等同**。组合键、宏可能使一个物理键产生多个 usage。

该功能只观察输入，不阻止原输入、不改键、不注入按键，也不提供物理开关事件。启动前已经按住的键可能暂不可知。权限丢失、设备移除或 helper 退出时，应用应以新的输入状态清理 held keys。重新订阅后按快照恢复按住显示，不把快照转换为新的业务按键动作；按键事件不持久化、不重放。旧版 Runtime 可能没有 `snapshot.input`，应用应兼容字段缺失。

## 数据模型

### Device

| 字段 | 类型及范围 |
| --- | --- |
| `deviceId` | 不透明字符串；真实 X1 当前形如 `x1-<平台外设ID>`，mock 为 `mock-x1` |
| `name` | 设备显示名称 |
| `connectionState` | 当前为 `disconnected`, `connecting`, `ready` |
| `batteryPercent` | `0..100` 整数或 `null` |
| `workMode` | `0..3` 整数或 `null`；UI 如显示模式 1–4，应加 1 |
| `lever` | `automatic`, `manual` 或 `null` |

断开或连接中会清空电量、模式与拨杆字段为 `null`。不要把 `null` 显示为 0%。当前 Device 不含固件版本、能力位、亮度或连接 flags。电量等更新依赖设备通知，Runtime 不持续轮询。

### Configuration

结构为 `{"device":{"brightnessPercent":60},"modes":[Mode]}`。亮度为 `1..100` 的整数；每个 Mode 包含 `mode`、`keys` 和 `lights`，完整结构见 `configuration.apply` 示例。

每模式恰好包含四个不同角色：`voice`、`approve`、`reject`、`submit`。这些名称标识键位，动作由 `action` 决定；角色名不代表 Runtime 已提供语音或 Hook 集成。每个键必须带 `description`，允许空字符串，最多 20 字节 ASCII，不接受中文描述。

每模式恰好包含下列九个不同灯效状态：

```text
notification permissionRequest postToolUse preToolUse sessionStart
stop taskCompleted userPromptSubmit sessionEnd
```

可选灯效名称为：

```text
off singleMove rainbowMove rainbowWave rainbowWaveSlow breathing
middleLight typingRipple comet scanBar pulseCenter warningBlink
successSweep blueThinking lowBattery chargingFlow approvalWait
```

客户端发送名称，不发送固件编号。保存状态灯效映射不会触发对应状态；当前没有 Hook 状态触发 RPC。

### Action 和 MacroStep

禁用动作：

```json
{"type": "disabled"}
```

快捷键动作：

```json
{"type": "shortcut", "modifiers": ["control", "shift"], "keyCode": 7}
```

`modifiers` 必填，可为空数组；仅允许不重复的 `control`、`shift`、`alt`、`gui`。`keyCode` 为 `0..255` 的整数 HID usage，不是 ASCII 或操作系统虚拟键码。`keyCode: 0` 表示无普通键，可用于纯修饰键组合。

宏动作包含 1–49 步，按原顺序执行：

```json
{
  "type": "macro",
  "steps": [
    {"type": "keyDown", "keyCode": 7},
    {"type": "delay", "delayMs": 30},
    {"type": "keyUp", "keyCode": 7},
    {"type": "releaseAll"},
    {"type": "noOp"}
  ]
}
```

`keyDown` / `keyUp` 的 `keyCode` 为 `0..255`；`delayMs` 为 `0..765` 的整数且必须能被 3 整除。`disabled`、`releaseAll`、`noOp` 不带额外参数。所有动作、步骤及配置对象都拒绝未知字段。

## 错误处理

受理前失败通过 RPC `error` 返回；受理后的执行失败通过 Operation 的 `status` / `errorCode` 返回，**不会再补发第二个 RPC 错误响应**。

### JSON-RPC 错误数字

| `error.code` | `error.data.code` |
| --- | --- |
| `-32700` | `PARSE_ERROR`，JSON 无法解析，响应 `id: null` |
| `-32600` | `INVALID_REQUEST`，请求信封非法，响应 `id: null` |
| `-32602` | `INVALID_PARAMS` |
| `-32601` | `UNSUPPORTED_CAPABILITY`，包括未知方法 |
| `-32000` | 其余业务错误 |

### 业务错误码

| 错误码 | 含义与处理 |
| --- | --- |
| `PERMISSION_DENIED` | 未认证或 token 不匹配；重新读取 discovery 并建立认证连接 |
| `PROTOCOL_MISMATCH` | 请求协议 major 不兼容；使用兼容客户端或 Runtime |
| `INVALID_PARAMS` | 参数字段、类型、范围或集合不合法；修正请求 |
| `INCOMPLETE_WRITE_GROUP` | 四键/九灯效组不完整，或缺少完整配置基线；补齐组或改用独立改键 |
| `UNSUPPORTED_CAPABILITY` | 方法未知或设备不支持所需能力；检查 hello 方法列表及设备适配情况 |
| `DEVICE_NOT_FOUND` | 目标设备未登记；重新核对设备快照 |
| `DEVICE_NOT_READY` | 设备未就绪或执行前失去可用状态；等待设备状态并重新核对基线 |
| `BUSY` | 设备 worker 占用、队列无法接收或持久记录达到上限；退避、等待终态；长期持续时需维护日志 |
| `BASE_CONFLICT` | token 已过期；重新读取并核对变更，不能静默覆盖 |
| `OPERATION_ID_CONFLICT` | 同一 operationId 已用于不同内容；核对原操作，为新的用户操作创建新 ID |
| `OPERATION_NOT_FOUND` | 当前 journal 没有此 ID；核对实例、discovery 路径和日志是否重置 |
| `DEVICE_TIMEOUT` | 硬件交换超时；查询终态及 effect，重新连接后核对实际影响 |
| `DEVICE_DISCONNECTED` | 硬件通信中断；等待重连，按原 operationId 查询结果 |
| `DEVICE_REJECTED` | 设备 ACK 报告拒绝；查看诊断并核对固件能力 |
| `DEVICE_INVALID_RESPONSE` | 设备状态响应无法解析；查看诊断并核对协议兼容性 |
| `STORAGE_ERROR` | 日志持久化失败；后续新写入停止，排除存储故障并核对原操作 |
| `RUNTIME_RESTARTED` | 未完成操作在重启后按影响未知恢复；不会自动重发 |
| `RUNTIME_STOPPING` | Runtime 正在退出，拒绝新的变更 |

HTTP 403、超出连接数、消息过大、认证超时、发送超时或事件积压等可能直接导致连接失败/关闭，不一定有 JSON-RPC 错误响应。客户端也要处理传输层失败。

## 限制与兼容性

| 项目 | 当前限制 |
| --- | --- |
| 客户端数量 | 同时最多 16 个连接 |
| 消息和帧 | 最大 1 MiB |
| 升级握手 | 最多 10 秒 |
| 首次认证 | 升级完成后最多 10 秒 |
| 单次服务端发送 | 最多 5 秒 |
| 事件缓冲 | 全局广播容量 256；消费者落后会被断开，需重新取快照 |
| 设备写入 | 每设备一个在途操作，worker 队列容量 1；不提供应用级批量排队 |
| 持久操作记录 | 最多 128 个已受理 ID，终态也计入；满后新写入返回 `BUSY` |
| X1 硬件命令 | 每次交换最多 4 秒；整次保存可远长于一个 RPC 超时 |
| 自动读回 | 总上限 45 秒；每次采集 53 个只读请求，连续两次一致才发布，最多采集三次 |

持久记录不会自动淘汰，也没有归档/删除 RPC。维护前应停止 Runtime 并核对客户端未决操作，不要在应用中直接删除运行中的日志或锁文件。

应用自己的 RPC 超时只表示“未收到响应”，不是取消操作。应将短时 RPC 等待与较长的写入进度等待分开；退出应用也不会取消已经受理的硬件写入。

新固件的动作、描述、亮度和灯效在连接时自动读回；完整可表示的读回可建立 configuration 基线。没有单独的配置主动刷新 RPC。OLED 有独立的主动查询、上传和暂停/继续接口，见 [OLED API](oled-api.md)，包括二进制块、文件传输通知及新增错误。当前不提供临时插件接管、按键注入、语音、固件升级、Hook 状态触发、历史事件重放或通用操作取消接口。不要把固件命令文档中的能力当作已经开放的 WebSocket API。

初版 [JSON Schema](../contracts/runtime-v1/schema.json) 和 fixtures 保持原 Studio 契约，**未完整收录 `keymap.apply`、输入监听及 readback 扩展**。接入现有版本时将本参考与对应扩展文档一起使用。当前硬件开发以 macOS 为主，Windows/Linux 真机行为尚未验收。

## 可运行的只读客户端

以下 Node.js 示例需要支持全局 `WebSocket` 的运行环境，无额外 npm 依赖。保存为 `runtime-readonly.mjs` 后运行 `node runtime-readonly.mjs`。它认证、订阅、读取当前设备配置并监听事件，不写硬件。通过 `AHAKEY_RUNTIME_DISCOVERY` 可指定 discovery 文件。

可先在 Runtime 仓库启动隔离模拟器：

```sh
cargo run --locked -- --mock --discovery /tmp/ahakey-api-demo/discovery.json
```

然后在另一个终端运行客户端：

```sh
AHAKEY_RUNTIME_DISCOVERY=/tmp/ahakey-api-demo/discovery.json node runtime-readonly.mjs
```

`/tmp/ahakey-api-demo` 若已存在，在 Unix 上必须是私有目录。模拟器不触碰蓝牙；它与同一 discovery 路径的真实 Runtime 仍共用实例锁。

```javascript
import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';

if (typeof WebSocket === 'undefined') throw new Error('此 Node.js 缺少全局 WebSocket');
const defaultPath = process.platform === 'darwin'
  ? join(homedir(), 'Library/Application Support/AhaKey/runtime/discovery.json')
  : process.platform === 'win32'
    ? join(process.env.LOCALAPPDATA, 'AhaKey/runtime/discovery.json')
    : join(process.env.XDG_RUNTIME_DIR ?? join(homedir(), '.local/state'), 'ahakey/discovery.json');
const info = JSON.parse(await readFile(process.env.AHAKEY_RUNTIME_DISCOVERY ?? defaultPath, 'utf8'));
const url = new URL(info.endpoint);
if (url.protocol !== 'ws:' || url.hostname !== '127.0.0.1' || !url.port ||
    url.pathname !== '/' || url.search || url.hash || url.username || url.password) {
  throw new Error('拒绝向非预期本机 endpoint 发送 token');
}

const ws = new WebSocket(info.endpoint);
const pending = new Map();
let serial = 0;
let subscriptionId;
let instanceId;
let sequence = 0n;

// 响应和事件共用一个接收入口。示例只打印事件；应用应在这里更新本地状态。
ws.addEventListener('message', ({ data }) => {
  const message = JSON.parse(data);
  if (message.method === 'runtime.event') {
    const event = message.params;
    if (event.subscriptionId !== subscriptionId || event.instanceId !== instanceId) return;
    const next = BigInt(event.sequence);
    if (next <= sequence) return;
    if (next !== sequence + 1n) {
      console.error('事件序号缺口；请重新连接并取快照');
      ws.close();
      return;
    }
    sequence = next;
    console.log('event', event.type, event.data);
    return;
  }
  const entry = pending.get(message.id);
  if (!entry) return;
  pending.delete(message.id);
  clearTimeout(entry.timer);
  if (message.error) entry.reject(new Error(message.error.data?.code ?? message.error.message));
  else {
    // 在接收订阅响应时安装边界，确保紧随其后的事件使用新订阅状态。
    if (entry.method === 'runtime.subscribe') {
      subscriptionId = message.result.subscriptionId;
      instanceId = message.result.snapshot.instanceId;
      sequence = BigInt(message.result.snapshot.sequence);
    }
    entry.resolve(message.result);
  }
});
ws.addEventListener('close', () => {
  for (const entry of pending.values()) {
    clearTimeout(entry.timer);
    entry.reject(new Error('连接关闭；重新读取 discovery 并认证订阅'));
  }
  pending.clear();
});
ws.addEventListener('error', () => console.error('WebSocket 连接错误'));

function rpc(method, params) {
  const id = String(++serial);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`RPC ${method} 超时；写入调用须查询原 operationId`));
    }, 10_000);
    pending.set(id, { method, resolve, reject, timer });
    try { ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params })); }
    catch (error) { clearTimeout(timer); pending.delete(id); reject(error); }
  });
}

const opening = new Promise((resolve, reject) => {
  ws.addEventListener('open', resolve, { once: true });
  ws.addEventListener('error', () => reject(new Error('升级失败')), { once: true });
  ws.addEventListener('close', () => reject(new Error('升级期间关闭')), { once: true });
});
try {
  await opening;
  const hello = await rpc('runtime.hello', {
    protocol: { major: 1, minor: 0 },
    client: { name: 'Runtime Readonly Example', version: '1', kind: 'cli' },
    token: info.token,
  });
  if (hello.protocol.major !== 1) throw new Error('不兼容的协议版本');
  for (const method of ['runtime.subscribe', 'configuration.get']) {
    if (!hello.methods.includes(method)) throw new Error(`缺少接口 ${method}`);
  }
  const { snapshot } = await rpc('runtime.subscribe', { topics: ['state'], cursor: null });
  console.log('snapshot', snapshot);
  for (const device of snapshot.devices) {
    try { console.log('configuration', await rpc('configuration.get', { deviceId: device.deviceId })); }
    catch (error) { console.error(error.message); }
  }
  console.log('正在监听；Ctrl-C 退出');
} catch (error) {
  console.error(error.message);
  ws.close();
  process.exitCode = 1;
}
process.once('SIGINT', () => ws.close());
```

这是单次连接的示例。产品客户端应在连接失败、关闭或序号缺口时加入退避重连、快照替换和未决操作恢复；不要打印 discovery token。现有 Rust 改键客户端见 [runtime-keymap.rs](../examples/runtime-keymap.rs)，默认预览，显式 `--apply` 才写入。

## 实现与相关资料

- [IPC 实现](../src/ipc/mod.rs)：认证、请求解码、分发与升级限制。
- [Runtime 实现](../src/runtime/mod.rs)：快照、事件、基线、幂等与操作执行。
- [Wire 模型](../src/wire.rs)、[配置模型](../src/configuration/mod.rs)、[改键模型](../src/configuration/keymap.rs)、[输入模型](../src/input/mod.rs)。
- [初版架构](runtime-v1.md)、[独立改键说明](keymap-api.md)、[输入监听集成](en/input-monitoring.md)。
- [固件联调手册](ble-integration-guide.md)、[调试日志](debug-mode.md)。

文档核对日期：2026 年 10 月 19 日。接口变化时应同步更新本参考；旧版固定 fixtures 不代表新增接口的完整清单。
