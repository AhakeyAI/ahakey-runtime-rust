# Runtime 自动打包与发布

工作流：[Runtime CD](../.github/workflows/release.yml)。Rust Build CI 检查代码；CD 构建、测试并验证可安装的发布产物。

| 操作 | 结果 |
| --- | --- |
| 推送到 `main` | 四平台开发安装包，保留在 Actions Artifacts 中 14 天 |
| Run workflow，`release_tag` 留空 | 构建所选分支的开发包，不发布 Release |
| 推送 `v版本号` 标签 | 全部平台通过后创建 GitHub Release，上传安装包和具体发布说明 |
| Run workflow，填入已有版本的 `release_tag` | 从原标签重建 Runtime，以当前工作流重新打包、更新已有 Release；不移动标签 |

## 平台与安装

| 平台 | 文件后缀 | 安装和启动 |
| --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin.dmg` | 拖动 `AhaKey Runtime.app` 到“应用程序”，然后打开 |
| macOS Intel | `x86_64-apple-darwin.dmg` | 同上；两种 macOS 均需要 13+ |
| Windows x64 | `x86_64-pc-windows-msvc.exe` | Windows 10+，运行安装向导，然后从开始菜单启动 |
| Linux x64 | `x86_64-unknown-linux-gnu.deb` | Ubuntu 22.04 或兼容的 Debian 系统，执行 `sudo apt install ./ahakey-runtime-*.deb`，再执行 `ahakey-runtime` |

Windows 安装到 `%LOCALAPPDATA%\Programs\AhaKey Runtime`，可在系统设置中卸载；CD 静态链接 MSVC CRT，无需另装 Visual C++ 运行库。
Linux 命令为 `/usr/bin/ahakey-runtime`，程序和清单位于 `/usr/lib/ahakey-runtime`，文档位于 `/usr/share/doc/ahakey-runtime`；通过 `sudo apt remove ahakey-runtime` 卸载。
Debian 包依赖使用 `dpkg-shlibdeps` 从实际二进制生成，另声明 BlueZ 和 D-Bus。

Runtime 是后台服务，没有设置窗口。macOS 打开后在后台运行，可通过“活动监视器”退出；Windows 打开控制台，Ctrl+C 可退出；Linux 在终端前台运行，Ctrl+C 可退出。安装包不设置开机自启，也不安装系统服务。

macOS 需要命令行参数时运行：

```sh
"/Applications/AhaKey Runtime.app/Contents/MacOS/ahakey-runtime-rust" --mock
```

macOS 的 `ahakey-input-monitor` 与 Runtime 同架构，放在同一 `Contents/MacOS` 目录，使用 `--input-monitor` 启用并授予输入监控权限。
macOS 产物尚无 Developer ID 签名和公证；Windows 安装程序尚无代码签名，系统可能拦截首次启动。集成 Studio 时由宿主统一签名。

## 校验

每个平台发布安装包、`.sha256` 和 `.build-info.json`；Release 另附汇总 `SHA256SUMS`。
包内 `BUILD-INFO.json` 记录版本、源码提交、平台、编译器及文件校验值；包外清单另记录安装包的哈希和大小。
重打包时 `sourceCommit` 仍是原标签的提交，`packagingCommit` 记录本次打包工具的提交。

CD 在各自系统上实际验证：

- macOS 挂载 DMG、复制 `.app`，校验主程序和 helper。
- Windows 静默安装，校验文件，运行后测试卸载。
- Linux 解包并校验文件和命令链接，再用 `apt` 安装、运行和卸载。
- 全部平台运行已安装程序的 `--help` 和 `--mock`，检查 discovery、监听端口及父进程关闭 stdin 后的正常退出。

每个安装包验证通过后才上传，全部平台通过后才发布。真机蓝牙、输入授权和设备持久化仍需设备验收。

## 发布新版本

1. 更新 `Cargo.toml` 的 `package.version`，同步 `Cargo.lock`。
2. 编写 `docs/releases/v版本号.md`，说明功能、安装方法、兼容范围及当前限制。发布流程使用这份说明，不再只依赖 GitHub 的自动提交链接。
3. 提交到 `main`，在该提交打标签，版本必须完全一致：

   ```sh
   git tag -a v0.1.0 -m "AhaKey Runtime v0.1.0"
   git push origin v0.1.0
   ```

4. 查看 Runtime CD，四个平台均通过后自动发布。

`v0.1.0-rc.1` 要求 Cargo 版本为 `0.1.0-rc.1`，自动标记为预发布；标签必须指向 `main` 已包含的提交。
标签触发的发布不覆盖已有 Release。

## 重打包已有版本

当 Runtime 源码无需改变、仅改安装格式或发布说明时：

1. 将打包工具和 `docs/releases/v版本号.md` 的更新提交到 `main`。
2. Actions → Runtime CD → Run workflow，选择 `main`，`release_tag` 填入 `v0.1.0` 等已有标签。
3. 工作流从原标签检出 Runtime，使用当前打包工具生成并验证安装包。
4. 全部通过后上传安装包和清单、更新发布说明，最后移除旧 `.zip` / `.tar.gz` 资产。

版本标签和 Runtime 源码保持原样。更新同名安装包会替换已有资产；只有手动填写版本标签的流程执行这一步。

## 本地打包

Apple Silicon 示例，需要 Rust、Swift 和 macOS 自带的 `hdiutil`：

```sh
MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --locked --release --target aarch64-apple-darwin
swiftc -O -swift-version 5 -target arm64-apple-macosx13.0 native/macos/InputMonitor.swift -o target/aarch64-apple-darwin/release/ahakey-input-monitor
python3 -B scripts/package-runtime.py --target aarch64-apple-darwin --label local-test
python3 -B scripts/verify-installer.py --target aarch64-apple-darwin --installer dist/ahakey-runtime-local-test-aarch64-apple-darwin.dmg
```

Windows 另需 Inno Setup 6；Linux 另需 `dpkg-dev` 和链接所需开发库。各平台在本系统打包。
默认写入被忽略的 `dist/`；同名包不覆盖，使用 `--output /新目录` 可重试。
本地脏工作区允许打包并记录 `sourceDirty=true`，CD 使用 `--require-clean`。
