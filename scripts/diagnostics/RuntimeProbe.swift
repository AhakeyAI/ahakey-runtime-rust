import Foundation

@main
struct RuntimeProbe {
    @MainActor
    static func main() async throws {
        let target = CommandLine.arguments.dropFirst().first { !$0.hasPrefix("--") }
        let client = IPCRuntimeClient()
        var latest: RuntimeDevice?
        client.onEvent = { event in
            if event.type == "device.changed", let device = try? event.data.decoded(RuntimeDevice.self), device.deviceId == target {
                latest = device
            }
        }
        let handshakeStart = DispatchTime.now().uptimeNanoseconds
        var session = try await client.connect()
        if CommandLine.arguments.contains("--latency"), let target {
            let handshakeMs = Double(DispatchTime.now().uptimeNanoseconds - handshakeStart) / 1_000_000
            var samples: [Double] = []
            for _ in 0..<20 {
                let start = DispatchTime.now().uptimeNanoseconds
                _ = try await client.configuration(deviceId: target)
                samples.append(Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000)
            }
            samples.sort()
            print(String(format: "IPC hello + subscribe: %.2f ms; cached configuration.get (20 calls): median %.2f ms, p95 %.2f ms, max %.2f ms", handshakeMs, samples[10], samples[18], samples[19]))
        }
        if CommandLine.arguments.contains("--reconnect"), let target {
            try await client.disconnectDevice(id: target)
            try await Task.sleep(nanoseconds: 500_000_000)
            try await client.connectDevice(id: target)
            for _ in 0..<100 where latest?.isReady != true {
                try await Task.sleep(nanoseconds: 200_000_000)
            }
            session = try await client.connect()
        }
        defer { client.disconnect() }
        let devices = session.subscription.snapshot.devices.filter { target == nil || $0.deviceId == target }
        for device in devices {
            print("Device \(device.deviceId): \(device.connectionState), battery=\(device.batteryPercent.map(String.init) ?? "unknown")")
        }
        guard !devices.isEmpty, devices.allSatisfy(\.isReady) else {
            print("FAIL: DEVICE_NOT_READY (no ready hardware session)")
            exit(1)
        }
        print("PASS: hardware session is ready")
    }
}
