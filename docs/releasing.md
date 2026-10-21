# Runtime 自动打包与发布

工作流：[Runtime CD](../.github/workflows/release.yml)。
原有 Rust Build CI 继续检查 `main/dev` 的推送和 PR；CD 独立构建和测试发布产物。

| 操作 | 结果 |
| --- | --- |
| 推送到 `main` | 四平台开发包，保留在本次 Actions 的 Artifacts 中 14 天 |
| Actions → Runtime CD → Run workflow | 为所选分支或标签构建开发包，不发布 Release |
| 推送 `v版本号` 标签 | 全部平台检查通过后，自动创建 GitHub Release 并上传发布包 |

## 平台与内容

| 平台 | Rust target | 文件格式 | 运行要求 |
| --- | --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` | `.tar.gz` | macOS 13+ |
| macOS Intel | `x86_64-apple-darwin` | `.tar.gz` | macOS 13+ |
| Windows x64 | `x86_64-pc-windows-msvc` | `.zip` | Windows x64、Microsoft Visual C++ x64 运行库 |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.tar.gz` | glibc 2.35+、libdbus-1.so.3、BlueZ 和 D-Bus 服务 |

包内有 release 版 Runtime、LICENSE、双语运行说明和 `BUILD-INFO.json`。
macOS 另附同架构的 `ahakey-input-monitor`，与 Runtime 放在同一目录，使用 `--input-monitor` 启用。
manifest 记录版本、提交、构建平台、编译器和包内文件 SHA-256；包外另有压缩包 `.sha256`。
GitHub Release 还提供汇总的 `SHA256SUMS`。

macOS 产物尚未做 Developer ID 签名或公证，集成 Studio 时由宿主对两个程序统一签名。
Windows 包也没有代码签名。当前流程不需要配置个人 Token 或签名凭证。

## 发布一个版本

1. 更新 `Cargo.toml` 的 `package.version`，同步 `Cargo.lock`，提交并合并到 `main`。
2. 在该提交上打标签，版本必须完全一致。例如 `Cargo.toml` 为 `0.1.0`：

   ```sh
   git tag -a v0.1.0 -m "AhaKey Runtime v0.1.0"
   git push origin v0.1.0
   ```

3. 在 GitHub Actions 查看 Runtime CD。四个平台均通过测试并完成打包后，才发布 Release。

`v0.1.0-rc.1` 这样的预发布标签要求 Cargo 版本为 `0.1.0-rc.1`，自动标记为 prerelease。
标签必须指向 `main` 已包含的提交。已有同名 Release 不会被覆盖；每个版本使用新标签。
格式、测试、macOS ARM64 Clippy、helper 编译或包校验失败时，不发布 Release。
发布产物通过编译和自动测试，不代表已完成真机蓝牙、输入权限或设备持久化验收。

## 本地验证打包

以 Apple Silicon 为例，在仓库根目录执行：

```sh
MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --locked --release --target aarch64-apple-darwin
swiftc -O -swift-version 5 -target arm64-apple-macosx13.0 native/macos/InputMonitor.swift -o target/aarch64-apple-darwin/release/ahakey-input-monitor
python3 scripts/package-runtime.py --target aarch64-apple-darwin --label local-test
```

默认写入被 Git 忽略的 `dist/`。同名包不覆盖，可以用 `--output /新的输出目录`。
本地脏工作区允许打包，manifest 中 `sourceDirty=true`；CD 使用 `--require-clean` 拒绝脏源码。
