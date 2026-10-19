import Foundation

// Compiled with the unmodified DTOs, protocol and IPC client from Studio's dev branch.
@main
struct StudioSmoke {
    @MainActor
    static func main() async throws {
        let client = IPCRuntimeClient()
        var events: [RuntimeEvent] = []
        client.onEvent = { events.append($0) }
        let session = try await client.connect()
        guard let device = session.subscription.snapshot.devices.first, device.isReady else { fatalError("Mock device not ready") }
        let document = try await client.configuration(deviceId: device.deviceId)
        guard var configuration = document.configuration else { fatalError("Missing simulator baseline") }
        configuration.device.brightnessPercent = 61
        configuration.modes = [configuration.modes.first!]
        // Exercise every action shape through Swift's actual Codable implementation.
        configuration.modes[0].keys[0].action = .disabled
        configuration.modes[0].keys[1].action = .shortcut(modifiers: ["gui"], keyCode: 0)
        configuration.modes[0].keys[2].action = .macro(steps: [
            RuntimeMacroStep(type: "keyDown", keyCode: 40),
            RuntimeMacroStep(type: "delay", delayMs: 15),
            RuntimeMacroStep(type: "keyUp", keyCode: 40),
            RuntimeMacroStep(type: "releaseAll"),
            RuntimeMacroStep(type: "noOp"),
        ])
        let request = ApplyConfiguration(operationId: UUID().uuidString, deviceId: device.deviceId,
            baseToken: document.baseToken, scope: .init(deviceFields: ["brightnessPercent"], modes: [configuration.modes[0].mode]), changes: configuration)
        let accepted = try await client.apply(request)
        guard accepted.operationId == request.operationId else { fatalError("Mismatched operation") }
        client.disconnect()
        let recovered = IPCRuntimeClient()
        _ = try await recovered.connect()
        var operation = try await recovered.operation(id: request.operationId)
        for _ in 0..<100 where !operation.isTerminal {
            try await Task.sleep(nanoseconds: 20_000_000)
            operation = try await recovered.operation(id: request.operationId)
        }
        guard operation.status == "completed" else { fatalError("Save failed: \(operation)") }
        let updated = try await recovered.configuration(deviceId: device.deviceId)
        guard updated.configuration?.device.brightnessPercent == 61, updated.baseToken != document.baseToken,
              updated.configuration?.modes.count == 4 else { fatalError("Incorrect saved scope") }
        let duplicate = try await recovered.apply(request)
        guard duplicate.status == "completed" else { fatalError("Idempotency failed") }
        try await recovered.disconnectDevice(id: device.deviceId)
        try await Task.sleep(nanoseconds: 100_000_000)
        try await recovered.connectDevice(id: device.deviceId)
        try await Task.sleep(nanoseconds: 100_000_000)
        let invalidated = try await recovered.configuration(deviceId: device.deviceId)
        guard invalidated.configuration == nil else { fatalError("Reconnected hardware must not expose stale configuration") }
        recovered.disconnect()
        print("PASS: Studio dev IPCRuntimeClient → Rust WebSocket: hello, subscribe, get, apply, operation recovery, disconnect/connect; all action encodings")
    }
}
