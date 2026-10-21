# Runtime packaging and releases

Workflow: [Runtime CD](../../.github/workflows/release.yml).
Existing Rust Build CI still checks pushes and PRs targeting `main/dev`; CD builds and tests its own release artifacts.

| Trigger | Result |
| --- | --- |
| Push to `main` | Four-platform development packages, retained as Actions artifacts for 14 days |
| Actions → Runtime CD → Run workflow | Development packages for the selected ref; no GitHub Release |
| Push a `v*` tag | Publish a GitHub Release only after every platform passes |

## Packages

| Platform | Rust target | Format | Runtime requirements |
| --- | --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` | `.tar.gz` | macOS 13+ |
| macOS Intel | `x86_64-apple-darwin` | `.tar.gz` | macOS 13+ |
| Windows x64 | `x86_64-pc-windows-msvc` | `.zip` | Windows x64 and the Microsoft Visual C++ x64 runtime |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.tar.gz` | glibc 2.35+, libdbus-1.so.3, BlueZ and D-Bus services |

Each package contains the release binary, LICENSE, bilingual instructions and `BUILD-INFO.json`.
macOS packages also contain `ahakey-input-monitor` for the same architecture, beside Runtime; enable it with `--input-monitor`.
The manifest records version, commit, target, compiler and file SHA-256 hashes. Each archive has a `.sha256` sidecar;
GitHub Releases also include an aggregate `SHA256SUMS` file.

macOS packages have no Developer ID signature or notarization. Sign both executables when integrating into Studio.
Windows packages are also unsigned. No personal token or signing credentials are needed for this workflow.

## Publish a version

1. Update `package.version` in `Cargo.toml`, synchronize `Cargo.lock`, and merge the commit into `main`.
2. Tag that commit with the exact Cargo version. For version `0.1.0`:

   ```sh
   git tag -a v0.1.0 -m "AhaKey Runtime v0.1.0"
   git push origin v0.1.0
   ```

3. Follow Runtime CD in GitHub Actions. All four platform jobs must pass before publication.

For `v0.1.0-rc.1`, Cargo must contain `0.1.0-rc.1`; the release is automatically marked as a prerelease.
Tags must point to commits already contained in `main`. Existing releases are not overwritten; use a new tag for each version.
Formatting, tests, macOS ARM64 Clippy, helper compilation or package verification failures block publication.
Automated checks do not replace Bluetooth, input-permission or device-persistence acceptance on real hardware.

## Package locally

For Apple Silicon, from the repository root:

```sh
MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --locked --release --target aarch64-apple-darwin
swiftc -O -swift-version 5 -target arm64-apple-macosx13.0 native/macos/InputMonitor.swift -o target/aarch64-apple-darwin/release/ahakey-input-monitor
python3 scripts/package-runtime.py --target aarch64-apple-darwin --label local-test
```

Output goes to the ignored `dist/` directory. Existing packages are not overwritten; use `--output /new/output/directory`.
Local dirty-source packages are marked `sourceDirty=true`; CD rejects them with `--require-clean`.
