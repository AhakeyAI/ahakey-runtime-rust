import Foundation
import IOKit.hid
import Darwin

// Read-only timing probe for one named keyboard. Never prints usages or typed text.
final class Timing {
    var downAt: [UInt32: UInt64] = [:]
    var previousUp: [UInt32: UInt64] = [:]
    var downCount = 0
    var upCount = 0
    var quickRepresses = 0
    var queueDelays: [Double] = []
    var timebase = mach_timebase_info_data_t()
    init() { mach_timebase_info(&timebase) }
    func milliseconds(_ ticks: UInt64) -> Double {
        Double(ticks) * Double(timebase.numer) / Double(timebase.denom) / 1_000_000
    }
    func receive(_ value: IOHIDValue) {
        let element = IOHIDValueGetElement(value)
        guard IOHIDElementGetUsagePage(element) == 7 else { return }
        let usage = IOHIDElementGetUsage(element)
        guard usage > 3 else { return }
        let timestamp = IOHIDValueGetTimeStamp(value)
        let now = mach_absolute_time()
        if now >= timestamp { queueDelays.append(milliseconds(now - timestamp)) }
        let pressed = IOHIDValueGetIntegerValue(value) != 0
        if pressed {
            guard downAt[usage] == nil else { return }
            downCount += 1
            if let previous = previousUp[usage], timestamp >= previous, milliseconds(timestamp - previous) < 50 {
                quickRepresses += 1
                print("Rapid same-key release/repress gap under 50ms (may be macro or bounce)")
            }
            downAt[usage] = timestamp
            print("DOWN \(downCount)")
        } else if let start = downAt.removeValue(forKey: usage), timestamp >= start {
            upCount += 1
            previousUp[usage] = timestamp
            print(String(format: "UP %d, held %.2f ms", upCount, milliseconds(timestamp - start)))
        }
        fflush(stdout)
    }
    func summary() {
        queueDelays.sort()
        print("SUMMARY down=\(downCount) up=\(upCount) unreleased=\(downAt.count) rapidRepresses=\(quickRepresses)")
        if !queueDelays.isEmpty {
            print(String(format: "Host callback queue lag median=%.2f ms max=%.2f ms (not physical-key end-to-end latency)", queueDelays[queueDelays.count/2], queueDelays.last!))
        }
    }
}
let product = CommandLine.arguments.dropFirst().first ?? "vibe code B1D2"
let duration = CommandLine.arguments.count > 2 ? Double(CommandLine.arguments[2]) ?? 30 : 30
let timing = Timing()
let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
IOHIDManagerSetDeviceMatching(manager, [kIOHIDProductKey: product] as CFDictionary)
IOHIDManagerRegisterInputValueCallback(manager, { context, result, _, value in
    guard result == kIOReturnSuccess, let context else { return }
    Unmanaged<Timing>.fromOpaque(context).takeUnretainedValue().receive(value)
}, Unmanaged.passUnretained(timing).toOpaque())
IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetCurrent(), CFRunLoopMode.defaultMode.rawValue)
let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
guard result == kIOReturnSuccess else {
    print(String(format: "HID_OPEN_FAILED 0x%08x; macOS may require Input Monitoring access", result))
    exit(2)
}
let devices = IOHIDManagerCopyDevices(manager) as? Set<IOHIDDevice> ?? []
guard !devices.isEmpty else { print("TARGET_KEYBOARD_NOT_FOUND"); exit(3) }
print("READY: read-only keyboard timing for \(Int(duration)) seconds. No key identities or text are logged.")
fflush(stdout)
CFRunLoopRunInMode(.defaultMode, duration, false)
timing.summary()
IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
