# master 固件：只修改四个按键

`examples/x1-keymap.rs` 使用现有 `ModelX1` 的 BLE 连接与逐条 ACK 确认，
向 master 固件写入四个普通字母键，无修饰键、无宏。
这是独立维护工具，尚未接入 Frontend 的保存按钮。

先退出 Studio 和 Runtime，让此工具独占设备。工具使用 Runtime 的 discovery
锁；若采用自定义 discovery 路径，运行时须保留相同的 `AHAKEY_RUNTIME_DISCOVERY`。
不要同时运行其他配置客户端。

在 Rust 仓库中先检查设备和当前模式（不会写入按键配置）：

```sh
cargo run --locked --example x1-keymap -- --keys dfjk
```

输出包含设备 ID、当前模式和计划发送的帧。确认输出后，将 ID 和模式代入：

```sh
cargo run --locked --example x1-keymap -- \
  --device-id x1-UUID --mode 0 --keys dfjk --apply
```

`--mode` 是固件编号 `0..3`。工具要求写入时设备仍处于指定模式；如果检查后
切换了模式，会拒绝写入。四个字母按固件按键索引 `0..3` 排列。
换成其他四个小写英文字母即可自定义，例如 `asdf`。

DFJK 的 HID usage 分别是 `07 09 0D 0E`。工具依次发送：

```text
AA BB 73 73 <mode> 00 07 CC DD
AA BB 73 73 <mode> 01 09 CC DD
AA BB 73 73 <mode> 02 0D CC DD
AA BB 73 73 <mode> 03 0E CC DD
AA BB 04 CC DD
```

master 的 `command_process` 会用 `0x73/0x73` 替换指定按键原有绑定，
`0x04` 调用 `save_key_bound_data()` 保存。其他模式、灯效、亮度、按键描述
不在写入命令中。设备端直接产生 HID 按下/松开事件，退出工具后映射仍可使用。

固件没有按键绑定读回命令，工具无法备份旧绑定，也不会建立 Runtime 的完整配置
基线或更新 Frontend 本地草稿。ACK 表示命令已获固件响应，不能证明实物输出
或断电保存成功。写入后在空白文本框逐个按键，再重启键盘复测。
若中途失败，之前的写入可能已经生效，工具会停止，不自动重试或回滚。
