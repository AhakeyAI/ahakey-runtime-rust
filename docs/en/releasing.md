# Runtime packaging and releases

Workflow: [Runtime CD](../../.github/workflows/release.yml). Rust Build CI checks the code; CD builds and verifies installable artifacts.

| Trigger | Result |
| --- | --- |
| Push to `main` | Four-platform development installers, retained in Actions for 14 days |
| Run workflow, empty `release_tag` | Development installers for the selected ref; no Release |
| Push a `v*` tag | Publish installers and curated release notes after every platform passes |
| Run workflow with an existing `release_tag` | Rebuild the original tagged source with current packaging tools and update that Release without moving its tag |

## Installation

| Platform | Filename suffix | Install and run |
| --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin.dmg` | Drag `AhaKey Runtime.app` to Applications, then open it; macOS 13+ |
| macOS Intel | `x86_64-apple-darwin.dmg` | Same; macOS 13+ |
| Windows x64 | `x86_64-pc-windows-msvc.exe` | Run Setup and launch from the Start menu; Windows 10+ |
| Linux x64 | `x86_64-unknown-linux-gnu.deb` | `sudo apt install ./ahakey-runtime-*.deb`, then `ahakey-runtime`; Ubuntu 22.04 or compatible Debian systems |

Windows installs per user to `%LOCALAPPDATA%\Programs\AhaKey Runtime`; uninstall from Settings. CD links the MSVC CRT statically, so the Visual C++ redistributable is not required.
Linux installs the command at `/usr/bin/ahakey-runtime`, payload at `/usr/lib/ahakey-runtime` and documentation at `/usr/share/doc/ahakey-runtime`. Remove it with `sudo apt remove ahakey-runtime`.
`dpkg-shlibdeps` determines library dependencies from the binary; BlueZ and D-Bus are also declared.

Runtime has no settings window. macOS runs it in the background; quit it with Activity Monitor. Windows opens a console; Ctrl+C exits. Linux runs in the terminal; Ctrl+C exits. Installation does not enable login autostart or install a system service.

To pass command-line options on macOS:

```sh
"/Applications/AhaKey Runtime.app/Contents/MacOS/ahakey-runtime-rust" --mock
```

macOS includes the matching `ahakey-input-monitor` beside Runtime in `Contents/MacOS`. Enable it with `--input-monitor` and grant Input Monitoring permission.
macOS packages have no Developer ID signature or notarization; Windows installers are unsigned. The OS may block the first launch. Sign executables together when integrating into Studio.

## Verification

Each installer has `.sha256` and `.build-info.json` sidecars. Releases include an aggregate `SHA256SUMS`.
The embedded `BUILD-INFO.json` binds payload hashes to the version, source commit, target and compiler. The external manifest also contains the installer hash and size.
When repackaging, `sourceCommit` remains the tagged commit and `packagingCommit` identifies the updated packaging tools.

Native CD jobs mount/copy the macOS app, silently install/uninstall Windows Setup, and extract/verify/install/remove the Debian package. Every platform verifies installed file hashes and runs `--help` and `--mock`, checking discovery, the listening port and clean shutdown after closing parent stdin.
All packages must pass before publication. Real hardware still needs Bluetooth, input-permission and persistence acceptance.

## Publish a version

1. Update `package.version` in `Cargo.toml` and synchronize `Cargo.lock`.
2. Write `docs/releases/vVERSION.md` with features, installation instructions, compatibility and limitations. The workflow publishes that document instead of only generating a Git history link.
3. Commit to `main`, then create and push the exact matching tag:

   ```sh
   git tag -a v0.1.0 -m "AhaKey Runtime v0.1.0"
   git push origin v0.1.0
   ```

4. Follow Runtime CD. All four platform jobs must pass.

`v0.1.0-rc.1` requires Cargo version `0.1.0-rc.1` and is published as a prerelease. Tags must identify commits already contained in `main`. Tag-triggered publication does not overwrite an existing Release.

## Repackage an existing release

Commit packaging changes and updated notes to `main`. Select Actions → Runtime CD → Run workflow, choose `main` and enter an existing tag such as `v0.1.0` in `release_tag`.
The workflow builds Runtime from that original tag, uses current packaging tools, and verifies every installer. It then uploads installers and manifests, updates release notes and removes old `.zip` / `.tar.gz` assets. Existing installers of the same name are replaced only by this explicit manual operation.
Neither the tag nor Runtime source is moved or rewritten.

## Package locally

Apple Silicon example (requires Rust, Swift and macOS `hdiutil`):

```sh
MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --locked --release --target aarch64-apple-darwin
swiftc -O -swift-version 5 -target arm64-apple-macosx13.0 native/macos/InputMonitor.swift -o target/aarch64-apple-darwin/release/ahakey-input-monitor
python3 -B scripts/package-runtime.py --target aarch64-apple-darwin --label local-test
python3 -B scripts/verify-installer.py --target aarch64-apple-darwin --installer dist/ahakey-runtime-local-test-aarch64-apple-darwin.dmg
```

Windows additionally requires Inno Setup 6; Linux requires `dpkg-dev` and the development libraries used for linking. Build installers on their respective OS.
Output goes to ignored `dist/`. Existing packages are not overwritten; use `--output /new/directory` to retry. Local dirty checkouts are marked `sourceDirty=true`; CD uses `--require-clean`.
