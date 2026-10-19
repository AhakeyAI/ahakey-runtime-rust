import Foundation
import IOKit.hid
import Darwin

// This helper is launched by the bundled Runtime. It only observes AhaKey HID
// reports, never consumes input, and never writes keystrokes to a log file.
final class InputMonitor {
    private var manager: IOHIDManager?
    private var devices: [String: String] = [:]
    private var pressed: [String: [UInt32: UInt64]] = [:]
    private var lastState: Data?
    private var requested = false
    private var timebase = mach_timebase_info_data_t()
    init() { mach_timebase_info(&timebase) }

    private func emit(_ type: String, _ data: [String: Any]) {
        guard let bytes = try? JSONSerialization.data(withJSONObject: ["type": type, "data": data], options: [.sortedKeys]) else { return }
        FileHandle.standardOutput.write(bytes)
        FileHandle.standardOutput.write(Data([10]))
    }

    private func state(_ status: String, _ permission: String) {
        let data: [String: Any] = ["status": status, "permission": permission,
            "devices": devices.keys.sorted().map { id in
                ["inputDeviceId": id, "name": devices[id]!, "pressedUsages": (pressed[id] ?? [:]).keys.sorted()] as [String: Any]
            }]
        let encoded = try? JSONSerialization.data(withJSONObject: data, options: [.sortedKeys])
        guard encoded != lastState else { return }
        lastState = encoded
        emit("state", data)
    }

    private func identity(_ device: IOHIDDevice) -> (String, String)? {
        guard let name = IOHIDDeviceGetProperty(device, kIOHIDProductKey as CFString) as? String,
              name.lowercased().hasPrefix("ahakey") || name.lowercased().hasPrefix("vibe code") else { return nil }
        var id: UInt64 = 0
        guard IORegistryEntryGetRegistryEntryID(IOHIDDeviceGetService(device), &id) == KERN_SUCCESS else { return nil }
        return ("hid-\(id)", name)
    }

    func attached(_ device: IOHIDDevice) {
        guard let (id, name) = identity(device) else { return }
        devices[id] = name
        publishDevices()
    }
    func removed(_ device: IOHIDDevice) {
        guard let (id, _) = identity(device) else { return }
        devices.removeValue(forKey: id)
        pressed.removeValue(forKey: id)
        publishDevices()
    }
    private func publishDevices() { state(devices.isEmpty ? "deviceUnavailable" : "listening", "granted") }

    func receive(_ value: IOHIDValue) {
        let element = IOHIDValueGetElement(value)
        guard IOHIDElementGetUsagePage(element) == 7,
              let (id, name) = identity(IOHIDElementGetDevice(element)), devices[id] != nil else { return }
        let usage = IOHIDElementGetUsage(element)
        guard (4...255).contains(usage) else { return }
        let down = IOHIDValueGetIntegerValue(value) != 0
        let timestamp = IOHIDValueGetTimeStamp(value)
        let previous = pressed[id]?[usage]
        guard down != (previous != nil) else { return }
        var held: Any = NSNull()
        if down {
            pressed[id, default: [:]][usage] = timestamp
        } else {
            pressed[id]?.removeValue(forKey: usage)
            if let previous, timestamp >= previous { held = milliseconds(timestamp - previous) }
        }
        emit("key", ["inputDeviceId": id, "name": name, "usage": usage, "pressed": down,
            "timestampMs": String(UInt64(milliseconds(timestamp))), "heldMs": held])
    }

    private func milliseconds(_ ticks: UInt64) -> Double {
        Double(ticks) * Double(timebase.numer) / Double(timebase.denom) / 1_000_000
    }

    func refresh() {
        let access = IOHIDCheckAccess(kIOHIDRequestTypeListenEvent)
        guard access == kIOHIDAccessTypeGranted else {
            close()
            let unknown = access == kIOHIDAccessTypeUnknown
            state(unknown ? "permissionRequired" : "permissionDenied", unknown ? "notDetermined" : "denied")
            if unknown && !requested {
                requested = true
                _ = IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
            }
            return
        }
        guard manager == nil else { return }
        let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
        // X1 vendor ID; also verify the product name in every callback.
        IOHIDManagerSetDeviceMatching(manager, [kIOHIDVendorIDKey: 2007] as CFDictionary)
        let context = Unmanaged.passUnretained(self).toOpaque()
        IOHIDManagerRegisterDeviceMatchingCallback(manager, { context, result, _, device in
            guard let context, result == kIOReturnSuccess else { return }
            Unmanaged<InputMonitor>.fromOpaque(context).takeUnretainedValue().attached(device)
        }, context)
        IOHIDManagerRegisterDeviceRemovalCallback(manager, { context, _, _, device in
            guard let context else { return }
            Unmanaged<InputMonitor>.fromOpaque(context).takeUnretainedValue().removed(device)
        }, context)
        IOHIDManagerRegisterInputValueCallback(manager, { context, result, _, value in
            guard let context, result == kIOReturnSuccess else { return }
            Unmanaged<InputMonitor>.fromOpaque(context).takeUnretainedValue().receive(value)
        }, context)
        IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), CFRunLoopMode.defaultMode.rawValue)
        let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        guard result == kIOReturnSuccess else {
            IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetMain(), CFRunLoopMode.defaultMode.rawValue)
            IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
            state("error", "granted")
            return
        }
        self.manager = manager
        for device in IOHIDManagerCopyDevices(manager) as? Set<IOHIDDevice> ?? [] { attached(device) }
        publishDevices()
    }

    func close() {
        if let manager {
            IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetMain(), CFRunLoopMode.defaultMode.rawValue)
            IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        }
        manager = nil
        devices.removeAll()
        pressed.removeAll()
    }
}

let monitor = InputMonitor()
// Runtime closes this pipe on exit, including crashes; no orphan input listener.
DispatchQueue.global().async {
    while !FileHandle.standardInput.availableData.isEmpty {}
    DispatchQueue.main.async { monitor.close(); exit(0) }
}
monitor.refresh()
let timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in monitor.refresh() }
RunLoop.main.run()
