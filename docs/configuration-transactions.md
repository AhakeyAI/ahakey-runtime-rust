# 配置写入的暂停、核验与续写

X1 的 `keymap.apply`、`configuration.patch`、`configuration.apply` 现在使用逐字段事务。受理仍沿用原请求和 operationId；`operation.pause` / `operation.resume` 控制同一任务，不新建写入请求。Operation 出现可选 `configurationProgress` 时表示该任务支持配置续写；旧型号/旧 journal 未启用事务的记录保留原有失败语义。

## 写入与暂停

亮度、单个键位动作、单个描述是各自的字段；当前固件灯效必须按一整个模式的九种状态写入，因此是一个不可分组。键位动作的清除旧层、设置新层两帧也不可在用户暂停时拆开。固件没有可回滚事务，已执行字段不会因暂停回滚。

Runtime 在第一条设备写入前持久化原始观察值、已确认字段和在途字段；每个字段执行前后更新 journal。暂停在当前完整字段完成后生效，剩余字段与保存命令不发送。关 Runtime 时同样请求暂停。退出 Studio 不会暂停 Runtime。

新事务失败进入 paused / unknown，保留原任务；基线冲突等写入前拒绝仍为 failed。Runtime 崩溃后在途事务恢复为 paused / unknown / RUNTIME_RESTARTED，禁止自动重放；显式继续前必须核验设备。

## 继续前核验

继续前通过型号适配器重新读取设备配置，逐个比较全部目标字段，完成所有比较后才允许任何写入：

- 当前值等于目标值：跳过该字段，不能仅凭旧 ACK 跳过。
- 当前值等于任务受理时的已知原值：允许续写。
- 当前值无法表示：停在 CONFIGURATION_UNVERIFIABLE。
- 当前值既不是原值也不是目标值，或原值未知且当前值不等于目标值：停在 CONFIGURATION_CONFLICT，保留外部修改。用户需显式读取、核对后创建新的修改任务。

若清除/设置动作两帧之间丢失连接，设备可能处于第三种中间值；不将它推断为本任务所有，仍按冲突处理。重发原写入请求只返回原记录；继续必须显式调用 operation.resume。

继续时若所有字段已经一致，只执行尚未得到确认的保存步骤。保存 ACK 丢失时读回不能证明 Flash 持久性，因此再次继续可重发一次保存命令，但不重写已一致字段。每次继续只执行一次计划；不会盲目重试失败的写入。

## 完成证据

`configurationProgress` 包含 fieldsTotal、fieldsAcknowledged、fieldsVerified、phase 和 saveAcknowledged。phase 为 writing / verifying / saving / paused / complete / failed。字段确认数和验证数分别表示 ACK/读回证据；saveAcknowledged 只表示保存命令的 ACK，不表示断电保留已验证。

受理时已有设备 readback 的任务，写完后必须再次读回验证全部目标字段，再发送保存命令。没有读回能力的旧固件仍可完成首次明确写入，但 fieldsVerified 为 0；暂停后的续写必须有读回证据。MockX1 没有硬件读回，所以首次写入只有 ACK 证据，续写将暂停为不可核验，不伪造读取值。

完整读回最多受适配器 45 秒和事务 50 秒超时约束。期间设备 worker 独占，暂停在本次读取结束后处理。该事务没有通用取消或自动回滚。设备真实保存持久性、暂停后的键盘输入恢复，以及蓝牙中断的实际表现仍须真机验收。

## 验证

`tests/configuration_transaction.rs` 覆盖字段边界暂停、原任务重启恢复、在途日志恢复、丢失字段/保存 ACK、外部冲突、完整模式的字段核验和 WebSocket pause/resume。`src/ahakey/x1/commands.rs::transaction_plan` 定义型号专属分组；Runtime 不解析硬件帧。
