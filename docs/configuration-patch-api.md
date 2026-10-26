# configuration.patch

供 Studio 独立编辑亮度与按键描述。客户端必须先检查 `runtime.hello.methods` 包含 `configuration.patch`，并通过 `configuration.get` 获取当前 `baseToken`。完整配置为 `null` 时仍可提交明确列出的字段。

```json
{
  "operationId": "patch-example",
  "deviceId": "x1-example",
  "baseToken": "base-example",
  "brightnessPercent": 57,
  "descriptions": [{ "mode": 2, "index": 1, "description": "Approve" }]
}
```

- 亮度可省略；提供时为 1–100 的整数。
- 描述数组可省略，最多 16 个不同的 `(mode, index)`；两者均为 0–3。描述不超过 20 个 ASCII 字节，空字符串表示清除该描述。
- 至少有一个字段。未知字段、重复位置、越界值均拒绝。此方法不接受灯效、键位动作或完整配置对象。
- X1 只发送本次指定的 `0x85` 亮度、`0x73/0x75` 描述命令，最后发送一次 `0x04` 保存。不发送未改字段、按键动作清理命令或 `0x84` 整组灯效。灯效独立写入需要进一步协议支持。
- 复用 operationId 幂等、baseToken 冲突检查、单设备串行队列、持久操作记录和事件顺序。相同请求返回原任务；相同 ID 不同内容返回 `OPERATION_ID_CONFLICT`。重连只能查询原任务，不自动生成新 ID 重写。
- 配置快照中已有的完整配置仅合并指定字段；没有完整基线时不补默认值。写入后旧 readback 被失效，不能视为新的物理读回。

返回 `{ "operationId": "patch-example", "status": "accepted" }`。最终状态通过 `operation.changed` 或 `operation.get` 获取；完成前发送 `configuration.changed`。X1 事务中指令或保存 ACK 丢失保留 `paused / unknown`，后续重发同一 ID 不重复执行。

支持按完整字段暂停、持久化任务和读回后续写，见 [配置事务](configuration-transactions.md)。`configurationProgress` 区分字段 ACK、读回核验和保存 ACK；`completed` 不单独证明 Flash 断电保持。描述实机显示、保存保持与故障恢复仍需硬件验收。

包含此类任务的新 journal 需要支持 `configuration.patch` 的 Runtime 读取；旧程序不能识别新增请求类型。替换开发 Runtime 前应保留原 journal，不把旧程序启动失败当作可清空记录的理由。

权威实现：`src/configuration/patch.rs`、`src/ahakey/x1/commands.rs`。schema：`contracts/runtime-v1/schema.json#/$defs/ConfigurationPatch`。回归测试：`tests/runtime/patch.rs`。
