# 语音输入与 AI 编程联动

实现日期：2026-10-26。适用：新版 Runtime + 配套 Tauri Studio，macOS 13+。

## 语音

Runtime 管理 `AhaKey Speech.app`，Swift 使用 AVAudioEngine 与 Apple Speech；录音不经过 WebView。
`ahakey-input-monitor` 的所选输入设备 F18（usage 109）按下/松开触发录音。支持 hold 和 toggle。
输入监听只观察，不拦截系统按键。使用者应确保 Voice 键是独立 F18，未同时绑定其他听写应用。
该来源是本次连接的 HID 设备 ID，不声称是 BLE deviceId 或物理键编号。

首次使用：Studio“系统与权限”启用后台输入观察并重启受管 Runtime；在“语音输入”请求麦克风、语音识别与辅助功能权限，选择输入设备并保存设置。
辅助功能只用于自动输入；转写本身需要麦克风与 Speech 权限。UI 试录只预览，不自动粘贴。
正常按键会话可在完成后向原前台应用的原焦点元素发送粘贴；焦点变化或密码输入框保留结果供复制。
`pasteSent` 只表示事件已发送，不声称目标应用已接受文字。剪贴板仅在仍由本次输出占有时恢复。
悬浮窗不抢焦点，显示实时文本、复制结果和取消/清除按钮。

状态流：idle → recording → finalizing → completed（可选 inserting → completed）；失败为 failed。
120 秒自动停止，最终结果最多等 12 秒，之后报告超时。过期会话回调被丢弃，重复最终结果不能重复输出。
Runtime 与 helper 的管道关闭会结束录音；退出 Studio 不结束 Runtime。
设置保存到私有 `*.voice.json`；HID 来源和 enabled 不跨 Runtime 重启保存，断开输入设备后重新选择。
音频、转写、输入焦点不写 journal 或诊断日志。转写留在本次内存会话，清除或重启后消失。
仅本机识别不可用时返回 VOICE_OFFLINE_UNAVAILABLE；允许联网时 Apple Speech 可能联网处理音频。
AhaType 云端文字整理与云账号不是当前 Apple Speech 实现的一部分，仍未接入。

新增 RPC（仍需 runtime.hello 认证；请求 ID 必须是字符串）：

| 方法 | 参数 |
|---|---|
| voice.get | `{}` |
| voice.configure | `{enabled,inputDeviceId,trigger,locale,autoInsert,onDeviceOnly}` |
| voice.control | `{action,sessionId}`；action 为 start/stop/cancel/clear/copy/permissions/refresh |

stop、cancel、clear、copy 必须带当前 sessionId。start 是显式试录；真正的按键触发由 Runtime 执行。
快照含 voice；voice.changed 推送完整视图，Studio 重连直接恢复，不重放开始录音。

构建：`cargo build --bins`；`python3 scripts/build-speech-helper.py --output 'target/debug/AhaKey Speech.app'`。
Studio 的 prepare:native 会编译并打包匹配的语音组件。预置 Runtime 必须带同目录 helper app 和 ahakey-hook。
开发使用 ad-hoc 签名；分发需匹配 Developer ID、麦克风 entitlement 与实际安装位置权限验收。

## AI 编程联动

`ahakey-hook` 是不执行工具命令的 stdin/stdout 适配器，通过私有 discovery 连接认证 Runtime。
Claude Code 和 Codex 的 PermissionRequest 支持逐次 allow/deny；defer 返回空对象，保留宿主权限流程。
Cursor beforeShellExecution/beforeMCPExecution 使用 allow/deny/ask；其他生命周期事件只更新状态。
不修改 approval_policy、不添加全局允许规则，不自动信任 Codex Hook。

Studio 中先关联在线键盘并启用；自动放行另有明确开关。每次自动批准前，由已有设备 worker 查询新鲜硬件状态，只有查询成功且拨杆为 automatic 才返回 allow。
已缓存的自动档、断连、查询失败、配置变化、Runtime 重启都不能授予新的自动批准。
手动批准/拒绝来自 Studio 的具体 requestId，或所选 HID 来源录入的独立批准/拒绝键。
同时有多个待审批请求时，实体按键必须先有明确选中的目标；仅一条时可直接操作。组合键不触发批准。
输入监听不会吞掉原键码，勿将复杂宏作为审批键。录入动作本身不批准请求。
设备运行模式或配置失效后停用联动，需重新核对来源；输入设备断开清除录入按键。

审批最多等待 60 秒；过期、停用、会话结束交回工具审批。进程重启不恢复待审批请求。
至多保留 32 个会话、64 条审批记录、16 条 pending，请求内容仅在内存中，不进入诊断日志。
工具工作状态来自生命周期事件，显示最近事件，不代表持续探测工具进程存活。

| 方法 | 参数 |
|---|---|
| hooks.get | `{}` |
| hooks.configure | `{enabled,deviceId,allowAutomatic,inputDeviceId,approveUsage,rejectUsage}` |
| hooks.event | `{eventId,tool,sessionId,event,cwd,toolName,summary}`；由适配器调用 |
| hooks.poll | `{requestId}`；返回 pending/allow/deny/defer |
| hooks.decide | `{requestId,decision}` |
| hooks.select | `{requestId,learning}`；learning 为 approve/reject/null |

快照含 hooks；hooks.changed 推送完整视图。关闭 Studio 保留 Runtime 中的联动；显式停用或 Runtime 重启结束启用状态。

在 Studio 生成工具配置，将 hooks 合并到项目文件，保留已有条目：

- Claude Code：`.claude/settings.json`
- Cursor：`.cursor/hooks.json`
- Codex：`.codex/hooks.json`，之后使用 `/hooks` 审查并信任定义。

也可运行 `ahakey-hook --config --tool claude --discovery /absolute/discovery.json`。
配置中的二进制路径与 discovery 路径均经过 shell quoting，不含认证 token。移动应用后重新生成配置。
用户未接入 Hook 时不会凭工具名称推断工作状态。宿主版本是否加载这些事件必须在其 Hook 调试界面确认。

官方协议核对（2026-10-26）：

- [Claude Code Hooks](https://code.claude.com/docs/en/hooks)
- [Cursor Hooks](https://cursor.com/docs/hooks)
- [Codex Hooks](https://learn.chatgpt.com/docs/hooks)
- [Apple Speech audio buffers](https://developer.apple.com/documentation/speech/sfspeechaudiobufferrecognitionrequest)
- [Apple on-device recognition](https://developer.apple.com/documentation/speech/sfspeechrecognizer/supportsondevicerecognition)

## 验证与剩余验收

自动测试覆盖按键来源、按住/点击录音状态、取消后迟到结果、试录不自动输出、断连取消、重启不保留文本；审批测试覆盖新鲜拨杆查询、查询失败、单次决定、多请求隔离和三种真实 Hook 二进制与认证 WebSocket 的回包。
UI 测试覆盖显式设置、录音结束的 sessionId、刷新后结果恢复、审批 requestId、连接丢失禁用和配置生成。

仍需真人/真机验证后才能宣告整体完成：

- 在实际安装的应用上授权 Speech；按住和点击 Voice 键说话，检查实时/最终文字。
- 中文、英文识别；联网与仅本机模式；麦克风切换、权限撤销、睡眠唤醒。
- 原焦点输入、焦点改变后保留结果、剪贴板恢复、退出 Studio 后语音继续可用。
- 各目标工具真实会话加载 Hook、收到工作状态与真实审批；实体拨杆手动/自动、实体批准/拒绝与断连。
- 应用内完整安装/卸载 Hook 的流程尚未实现，目前提供配置生成与手动合并接入。
- 云端账号/AhaType 整理、工具状态驱动硬件灯光属于额外集成，当前尚未实现。


## 2026-10-26 应用包验证记录

本地开发包：`../ahakey-studio/src-tauri/target/debug/bundle/macos/AhaKey Studio.app`。
已重新打包语音组件、Hook 适配器和配套 Runtime，仍使用开发签名。

已执行并通过：

- Studio 前端生产构建、28 项模型测试、23 项浏览器界面测试（本机 Chrome）。
- Studio 原生测试（17 项库测试、4 项 supervisor 测试）及 Clippy。
- Runtime 语音状态机 5 项测试、语音集成 2 项测试、Hook 集成 3 项测试及 Clippy。
- 新应用包隔离 Mock 启动、退出 Studio 后 Runtime 存活、显式停止释放 discovery。
- 随包 Speech 签名验证、权限只读查询、stdin 关闭后退出；没有开启录音。
- 随包三种工具的 Hook 配置生成；包内 Runtime 公布语音和 Hook RPC。

本次原生查询：麦克风 granted，Speech notDetermined，辅助功能 true。安装位置或签名改变后需要以应用实际查询为准。
用户选择稍后进行真人／真机验收；本记录不把 Mock 与状态机测试当作实际识别或真实工具审批验收。

### 稍后验收步骤

1. 打开上述开发包。如仍附着旧 Runtime，在“系统与权限”确认无任务后关闭并重新启动受管后台。
2. 启用后台输入观察并重启 Runtime；在“语音输入”请求权限，授权后刷新状态。选择当前键盘输入来源并保存；Voice 键须发送 F18。
3. 先“开始试录”，说一句话并“结束录音”，核对文字。再分别测试按住和点击模式；打开 TextEdit，启用自动输入，验证松开后文字进入原输入框。
4. 验证取消录音、改换焦点后保留结果、复制结果、退出 Studio 后仍能按键录音。Runtime 重启后需重新启用。
5. 在“AI 编程联动”选择在线键盘并启用，生成目标工具的配置，手动合并到项目文件并重启该工具会话；Codex 使用 `/hooks` 审查和信任。
6. 在专用测试项目触发一次本来需要审批的简单命令，确认 Studio 显示对应命令与项目；测试批准、拒绝、交回工具确认，以及实体批准／拒绝键。
7. 明确开启自动审批后，分别测试拨杆手动档与自动档；断开键盘后确认不再自动批准。多请求并行时核对选中的按键目标。
