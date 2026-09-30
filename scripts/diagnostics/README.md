# Device diagnostics

These tools do not change keyboard mappings or firmware.

For new firmware integration, Debug builds print diagnostics by default: run
`cargo run --locked` without Studio to inspect raw BLE frames, command timing,
state changes, and IPC outcomes. Release builds require `--debug`;
`--debug-log NEW_FILE` redirects diagnostics to a file. Set `AHAKEY_DEBUG=0`
to disable the Debug build default. See [debug mode](../../docs/debug-mode.md) for setup and examples.

## Runtime and BLE session

```sh
python3 scripts/diagnostics/check-device.py 'x1-DEVICE_ID'
python3 scripts/diagnostics/check-device.py 'x1-DEVICE_ID' --latency
```

The latency probe measures the local IPC handshake and cached `configuration.get`, not physical-key latency. `--reconnect` explicitly disconnects/reconnects the chosen device and invalidates its configuration baseline. It is a regression check for the macOS stale-peripheral reconnect failure. The scripts compile the unmodified Swift IPC client from the sibling Studio dev branch; use `--studio PATH` to override that repository.

## Physical keyboard HID reports (macOS)

```sh
swiftc scripts/diagnostics/HIDTiming.swift -o /tmp/ahakey-hid-timing
/tmp/ahakey-hid-timing 'vibe code B1D2' 30
```

Use the actual keyboard product name. macOS must grant Input Monitoring access to the responsible terminal/application. `0xe00002e2` is `kIOReturnNotPermitted`; it is not evidence of a keyboard failure. Grant access through System Settings if you choose to run the probe, then restart the responsible terminal/application as requested by macOS.

The probe opens the selected device for shared, read-only monitoring; it does not seize the keyboard, inject input, or print key identities or text. In a blank text editor, test one simple, non-macro physical key with five separate taps approximately one second apart.

Output contains counts of key-down/up transitions, durations, same-key release/repress gaps under 50 ms, and OS callback queue timing. Rapid transitions can be an intentional macro and must not automatically be labelled contact bounce. A long callback delay is downstream of the HID event timestamp. A short callback delay does not establish short end-to-end latency: physical press time and firmware processing time are not available here.

For an isolation comparison, change one owner at a time: Rust Runtime, then the legacy AhaKey Agent. Merely closing the Studio window does not stop a launchd-managed Agent. Record which services were running and restore their prior state after testing.
