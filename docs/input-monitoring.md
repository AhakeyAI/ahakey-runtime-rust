# macOS input monitoring

Studio bundles two executables in `Contents/MacOS`: `ahakey-runtime-rust` and
`ahakey-input-monitor`. The latter is built from `native/macos/InputMonitor.swift`
and uses macOS IOKit. Rust remains the IPC state owner and supervises the adapter.

The app launches Runtime with `--input-monitor --parent-stdin`. The monitor checks
`IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)` and requests access once if the
system reports an undetermined permission. Denied access is displayed in Studio
with an Input Monitoring settings button. Permission is checked every second;
access revocation closes the HID manager and clears held keys. macOS may require
the user to quit and reopen the signed app after granting access. Do not reset TCC
to force repeated prompts. The app and both helpers must be signed consistently.

The adapter matches the current X1 HID vendor ID `2007`, then verifies an `AhaKey`
or `vibe code` product-name prefix before accepting a device or event. It observes
keyboard usage page 7, usages 4–255, including modifier keys. It neither seizes the
keyboard nor suppresses, remaps, or injects input. Other vendor IDs need an explicit
device profile before support can be claimed.

## IPC extension

The existing authenticated `runtime.subscribe` state subscription has an optional
`snapshot.input` field. Older clients may ignore it; newer clients must handle its
absence when connecting to an older Runtime. The vendored Studio v1 fixtures remain
unchanged. This additive extension introduces no mutation method.

```json
{
  "status": "listening",
  "permission": "granted",
  "devices": [{
    "inputDeviceId": "hid-12345",
    "name": "AhaKey 505D",
    "pressedUsages": [109]
  }]
}
```

`input.changed` carries that same object. Status is `disabled`, `starting`,
`permissionRequired`, `permissionDenied`, `deviceUnavailable`, `listening`, or
`error`. Permission is `unknown`, `notDetermined`, `denied`, or `granted`.
`input.key` carries:

```json
{
  "inputDeviceId": "hid-12345",
  "name": "AhaKey 505D",
  "usage": 109,
  "pressed": false,
  "timestampMs": "123456",
  "heldMs": 1042.5
}
```

Both use the existing `runtime.event` envelope and global sequence. Applying a
key event and taking a new subscription snapshot use the same Runtime state lock.
The snapshot therefore restores keys observed as held without replaying business
actions. Helper loss, permission loss and device removal clear live state. A slow
IPC client reconnects using the existing event-overflow behavior.

`inputDeviceId` is a macOS HID registry identity for this connection, not a BLE
device ID or physical key role. `timestampMs` is a decimal string derived from the
HID monotonic timestamp, not UTC or physical switch timing. `heldMs` is null on
press and measures the observed HID down/up interval on release. Combinations and
macros can generate multiple usages for one physical key. Current state is based
on observed transitions; a key already held before monitoring starts may not be
known until the next HID report.

Raw events and held keys are volatile and do not enter the durable operation
journal. Studio keeps at most 12 recent events in memory. A subprocess stdin pipe
ties each helper to its owner: app exit or crash closes Runtime's pipe, and Runtime
exit closes the monitor's pipe. Runtime preserves its normal accepted-write drain
before exiting. The mock mode never opens a real input monitor.

## Validation

Run `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`,
and the Studio Frontend test scheme. Native compilation and signed app packaging
are checked by the Studio build. Real first-run TCC presentation, granting access,
device events and revocation must also be checked interactively on the signed app;
unit tests do not reset permissions or treat a simulator as hardware evidence.
