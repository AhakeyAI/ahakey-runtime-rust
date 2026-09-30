# Runtime 独立改键

`keymap.apply` 通过正在运行的 Runtime 写入指定模式的一到四个键。
它复用设备 worker、ACK 确认、幂等 operationId、操作查询和持久化。
不要求完整配置基线或亮度读回；不会发送灯效、亮度或描述写入命令。

先调用 `configuration.get` 获取当前 `baseToken`，即使 `configuration` 为 null
也能使用 token。它用于拒绝旧会话和并发修改，不表示完整配置已知。

```json
{
  "jsonrpc": "2.0",
  "id": "remap-1",
  "method": "keymap.apply",
  "params": {
    "operationId": "unique-id-for-this-write",
    "deviceId": "x1-DEVICE-ID",
    "baseToken": "from-configuration.get",
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

API 的 mode 为 0..3，index 为 0..3；上例是 Studio 的模式 3、四键 DFJK。
每项支持已有的 shortcut / macro / disabled 动作，验证规则与完整配置一致。
只替换列出的键；未列出的键、其他模式、描述和灯光配置保留。
键位身份是固件索引，不是监听到的 HID usage。

返回 `{operationId,status}`，完成结果通过 `operation.get` 或 `operation.changed`
查询。同 ID 同内容返回原操作；同 ID 不同内容拒绝。键顺序和修饰键顺序会归一化。
修改设备时独占原有 worker，完整配置写入、改键和连接操作之间互斥。

X1 对每个指定键先清空互斥动作层，再写入新动作，最后发送 `0x04` 保存。
只有每个 ACK 成功才返回 completed。失败不自动重发，可能部分生效，结果为 unknown。
重启会将未完成操作记为 failed / RUNTIME_RESTARTED，绝不重放。
旧版完整配置操作日志可由新版本读取；写入新类型日志后不要降级运行旧 Runtime。

成功后 baseToken 更新；若原本有完整配置，只更新其中对应键的动作。
原本未知的完整配置仍为 null，不伪造灯光或其他按键的值。
固件未提供绑定读回，因此 completed 只证明命令和保存 ACK，实际按键输出和断电
持久性仍需真机检查。此接口是直接替换指定键位，不是读取/备份旧绑定。

## 使用正在运行的 Runtime

保持 Studio/Runtime 运行。以下客户端只走 IPC，不会另开蓝牙连接。
CLI 使用 Studio 的 1..4 模式编号，默认仅预览：

```sh
cargo run --locked --example runtime-keymap -- \
  --device-id x1-DEVICE-ID --mode 3 --keys dfjk
```

确认写入时添加 `--apply`。客户端自动取得当前 token，打印 operationId，
提交一次并等待终态。连接中断或超时后应查询该 ID，不能盲目换新 ID 重试。
`AHAKEY_RUNTIME_DISCOVERY` 可选择自定义 discovery 路径。

Studio 现有完整配置保存按钮仍调用 `configuration.apply`；接入此独立接口的 UI
应单独提供“保存键位”，不把只保存键位显示成完整配置保存成功。
