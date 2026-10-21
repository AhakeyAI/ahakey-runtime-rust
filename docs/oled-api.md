# OLED 图片上传接口

上位机选择、裁剪和预览图片；Runtime 解码图片、编码 RGB565，并独占 BLE 上传。本文描述已实现的接口。运行示例见 [oled-upload.mjs](../examples/oled-upload.mjs)。发现、认证、订阅与通用错误格式沿用 [WebSocket 参考](websocket-api.md)。

## 当前设备支持

当前实现针对远端 [Fireware-harness `70ca334f65d73b6a2f20a0598e55c6da59f26526`](https://github.com/AhakeyAI/AhaKey-X1-hardware-source/tree/70ca334f65d73b6a2f20a0598e55c6da59f26526) 的 CP2 资源协议，profile 为 `x1-cp2-resource-v1`。Runtime 同时检查 catalog 和实际 `0x88` 结构化读回；相同 catalog 或通用成功 ACK 不足以启用上传。配置读回也使用该分支的 `0x87/0x88` 协议。

| 项目 | 限制 |
| --- | --- |
| 源文件 | PNG、JPEG、GIF，最多 2 MiB；APNG 不支持 |
| 画面 | 每帧恰好 160×80；Runtime 不自动缩放或裁剪 |
| 默认动画 | 每模式最多 8 帧 |
| 状态动画 | 每资源最多 12 帧 |
| 播放 | 循环播放，固定 100ms/帧；多帧 GIF 必须与此一致 |
| 像素 | 透明部分合成到黑底，RGB565 大端；每帧 25,600 像素字节，后补 3,072 个零字节 |

超帧数、变速/非 100ms 动画、明确指定有限循环次数的多帧动画会被拒绝。上位机应让用户预览并主动转换后重新提交，Runtime 不丢帧或改速。

## 调用顺序

1. `runtime.hello` 认证，`oled.get` 查询设备 profile 与当前资源。
2. `resource.begin` 创建本机文件接收任务。
3. WebSocket 二进制消息逐块发送文件，每块等待确认。
4. `resource.finish` 校验 SHA-256、解码并保存资源，返回 `resourceId`。
5. `configuration.get` 取得当前 `baseToken`。
6. `oled.upload` 指定资源与设备目标位置，返回持久操作 ID。
7. 通过 `operation.changed` 或 `operation.get` 跟踪设备上传，等待 `completed` 或 `paused` / `failed`。

传文件、完成资源准备都不写设备；只有 `oled.upload` 执行设备写入。资源可跨客户端连接和 Runtime 重启使用。

## `resource.begin`

```json
{"jsonrpc":"2.0","id":"r1","method":"resource.begin","params":{"byteLength":12345,"sha256":"64位小写十六进制的源文件SHA256"}}
```

成功结果：

```json
{"uploadId":"e8797433-9784-4ee7-b906-751746b7a874","chunkBytes":65536}
```

每个 WebSocket 连接同时允许一个接收任务。任务归该连接所有，断开后丢弃未完成文件；已完成的资源继续保留。文件长度必须为 1…2,097,152 字节。hello 的 `limits` 同时提供 `messageBytes`、`resourceBytes` 和 `resourceChunkBytes`。

### 二进制块

一个 WebSocket binary message：

| 字节位置 | 内容 |
| --- | --- |
| 0…15 | `uploadId` 的 16 个 UUID 原始字节，按字符串去掉连字符后的十六进制顺序 |
| 16…19 | 文件 offset，u32 小端 |
| 20…末尾 | 1…65,536 字节源文件内容 |

offset 必须等于此前已确认的累计字节数，不能超出声明长度。块成功后在同一连接收到通知：

```json
{"jsonrpc":"2.0","method":"resource.progress","params":{"uploadId":"e8797433-9784-4ee7-b906-751746b7a874","receivedBytes":65536}}
```

错误通知为 `resource.error`，`params` 包含 `uploadId` 与字符串 `code`。无活动接收任务时 uploadId 为 null。错误块不追加数据，可以纠正后重新发送；不能未确认就直接发送下一块。这两种通知属于连接内文件传输确认，不使用 `runtime.event` 的状态序号。

## `resource.finish` / `resource.abort`

两者参数均为：

```json
{"uploadId":"e8797433-9784-4ee7-b906-751746b7a874"}
```

`finish` 要求已收到完整文件，SHA-256 与声明相同，图片符合上述限制。解码在后台线程进行，不阻塞其他客户端。成功结果示例：

```json
{"resourceId":"64位编码后资源的SHA256","width":160,"height":80,"frameCount":1,"intervalMs":100,"byteLength":28672,"crc32":510799585}
```

resourceId 标识完整编码后内容，与源文件 sha256 可以不同。相同画面会去重。`finish` 失败后该接收任务结束，重新 begin；`abort` 丢弃当前未完成文件，成功返回 `{}`。

资源保存在 journal 同名的 `.resources` 私有目录中，容量上限 64 MiB，满时返回 `RESOURCE_STORE_FULL`。当前没有资源删除/垃圾回收 RPC；维护时先停止 Runtime 并核对暂停操作所引用的文件。资源哈希仅为内容身份，不表示键盘上已有该资源。

## `oled.get`

参数为 `{"deviceId":"x1-DEVICE_ID"}`。通过设备 worker 串行查询；设备忙时返回 BUSY，断开时返回 DEVICE_NOT_READY。不拿缓存冒充新读取结果。

```json
{
  "profile":"x1-cp2-resource-v1",
  "width":160,"height":80,"intervalMs":100,
  "entries":[
    {"target":{"kind":"modeDefault","mode":0},"valid":false,"writeState":"empty","frameCount":0,"byteLength":0,"crc32":0,"maxFrames":8}
  ]
}
```

实际 entries 为全部 19 个位置：每个模式先默认动画、再 working/waiting/done，最后三个 multiState。`writeState` 为 empty / writing / valid / invalid。valid=false 时完整性字段为零；空位置可能播放固件内置画面，这不表示用户上传了资源。此接口读取元数据，不导出原始像素或 GIF。

目标类型：

```json
{"kind":"modeDefault","mode":0}
{"kind":"modeState","mode":0,"state":"working"}
{"kind":"multiState","state":"done"}
```

mode 为 0…3；state 为 working / waiting / done。上位机不传 Flash 地址、槽位或 BLE 指令。

## `oled.upload`

```json
{
  "operationId":"客户端生成并保留的唯一ID",
  "deviceId":"x1-DEVICE_ID",
  "baseToken":"来自configuration.get",
  "resourceId":"resource.finish返回的ID",
  "target":{"kind":"modeDefault","mode":0}
}
```

结果格式与配置写入相同：`{"operationId":"…","status":"accepted"}`。同 ID 相同请求返回已有操作，不重复执行；同 ID 不同请求返回 OPERATION_ID_CONFLICT。它不要求完整配置基线，且只修改目标资源。其他模式图片、键位、描述、灯效和亮度不改动。

Runtime 在受理时保留设备执行权，并持久保存操作和资源引用，再经同一个 worker 执行。设备 profile 查询、上传、配置写入不会相互插入命令。上传时设备普通连接状态仍可能为 ready，但操作为 running，其他设备操作返回 BUSY；上位机应显示“上传中，部分控制暂不可用”。

设备流程：核验 profile、读取目标元数据 → 已匹配则直接成功 → `0x8A begin` → 顺序 `0x8B chunk`（最多 48 字节）并逐条等待 ACK → `0x8C commit` → 用 `0x88` 重新核对 valid、帧数、字节数、CRC。固件映射目标位置，Runtime 不使用旧 70 帧布局或旧裸数据特征写入。

`operation.progress` 按已 ACK 的字节更新，commit 与最终核验之前最多 0.99。`completed / effect=complete` 表示提交成功且设备元数据匹配；固件提交路径包含其保存检查。此结果不代替物理 Flash 数据独立读回或断电保持验收。

成功另发 `runtime.event`，type 为 `oled.changed`，data 为 `{deviceId,target,resourceId}`。该事件说明本次目标操作成功；全部元数据仍通过 `oled.get` 主动查询，不自动覆盖 Studio 草稿。

成功后设备级 baseToken 会更新，下一次新写入须重新调用 configuration.get；既有键位/灯效配置内容保持原样。同一 operationId 的幂等重试仍返回原操作，不受旧 token 影响。

## 暂停、断线与继续

`operation.pause` 和 `operation.resume` 参数均为 `{"operationId":"…"}`，返回当前完整 Operation。仅适用于 OLED 操作。

- pause 是异步请求：当前命令等待结束后停止后续发送，断开上传连接，让固件中止连接所属事务。操作落为 paused；已经提交完成的操作保持 completed。
- 暂停/失败后 Runtime 重连并查询资源状态，确认控制链可用后才重新报告 ready；若恢复失败，继续显示 disconnected/connecting，不能把 paused 理解成设备已恢复。
- 设备写入开始后，单条命令连续 4 秒未获得回复便停止本轮发包，进入读回核对及有界重试；无法核对或重试用尽时暂停。仍有持续 ACK 的慢上传不设总时长截止。
- 写入开始后的断线、核验失败，以及 Runtime 意外退出会保留任务，结果标为 unknown；重启后不会自动写入。写入前能力检查失败则为 failed/none，不会阻塞旧设备后续改键。
- 用户主动 resume 时等待设备 ready，先读回目标。帧数、长度和 CRC 已匹配则直接完成；不匹配时重新传这一个资源，不重写其他项目。不支持在动图内部从断点继续。
- 暂停操作及资源可跨 Runtime 重启继续。重复 resume 一个 accepted/running/completed 操作只返回现状，不再启动第二次上传。
- 写入超时或断线后自动重连并读回本次目标：已匹配直接成功，明确不匹配才重传这一资源；每次提交或用户主动继续最多尝试 3 次（首次 + 2 次重试）。读回失败或次数用尽时暂停，等待用户主动继续，不盲目重发。重传动图时进度重新按本次已确认字节计算。

应用退出或 WebSocket 断开不自动暂停设备操作；要暂停须显式调用 pause。Runtime 正常关闭时请求暂停正在上传的图片，再结束连接。pause 不承诺恢复目标原图或回滚已写入内容。

新增错误：`RESOURCE_UPLOAD_NOT_FOUND`、`INVALID_RESOURCE_CHUNK`、`RESOURCE_INCOMPLETE`、`RESOURCE_HASH_MISMATCH`、`INVALID_IMAGE`、`UNSUPPORTED_IMAGE_FORMAT`、`IMAGE_DIMENSIONS`、`TOO_MANY_FRAMES`、`INCOMPATIBLE_FRAME_INTERVAL`、`INCOMPATIBLE_LOOP_COUNT`、`RESOURCE_STORE_FULL`、`RESOURCE_NOT_FOUND`、`RESOURCE_CORRUPT`、`RESOURCE_ERROR`、`RESOURCE_VERIFY_FAILED`、`UPLOAD_PAUSED`、`OPERATION_NOT_PAUSED`。未识别硬件协议返回 UNSUPPORTED_CAPABILITY。

## 验证范围

已覆盖实际 WebSocket 多块 GIF 传输、资源解码、Mock 设备上传、中途暂停与重启继续、提交 ACK 丢失后的核对成功、三次重试上限及旧设备改键兼容；协议用远端未修改的 C 事务实现生成 CRC/布局样例，校验完整 Rust 发包序列。这不等于已在真机验证上传、显示方向、断线恢复或断电保持；这些仍需要目标固件实机验收。
