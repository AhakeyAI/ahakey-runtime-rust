import AppKit
import AVFoundation
import Speech
import ApplicationServices

// Runtime owns this process through stdin. No recording or transcript is written to disk.
final class SpeechHelper: NSObject {
    private var engine: AVAudioEngine?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var sessionID: String?
    private var transcript = ""
    private var finalizing = false
    private var finished = false
    private var targetPID: pid_t?
    private var targetElement: AXUIElement?
    private var deadline: Timer?
    private var panel: NSPanel?
    private var label: NSTextField?
    private var lastPartialAt = Date.distantPast

    private func emit(_ type: String, _ fields: [String: Any] = [:]) {
        var value = fields
        value["type"] = type
        if let sessionID { value["sessionId"] = sessionID }
        guard let data = try? JSONSerialization.data(withJSONObject: value) else { return }
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([10]))
    }
    private func microphoneStatus() -> String {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: return "granted"
        case .notDetermined: return "notDetermined"
        default: return "denied"
        }
    }
    private func speechStatus() -> String {
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized: return "granted"
        case .notDetermined: return "notDetermined"
        default: return "denied"
        }
    }
    func permissions() {
        emit("permissions", ["microphone": microphoneStatus(), "speech": speechStatus(),
                             "accessibility": AXIsProcessTrusted()])
    }
    private func requestPermissions() {
        AVCaptureDevice.requestAccess(for: .audio) { _ in
            SFSpeechRecognizer.requestAuthorization { _ in
                DispatchQueue.main.async {
                    let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
                    _ = AXIsProcessTrustedWithOptions(options)
                    self.permissions()
                }
            }
        }
    }
    func command(_ value: [String: Any]) {
        guard let name = value["command"] as? String else { return }
        if name == "permissions" { requestPermissions(); return }
        if name == "refresh" { permissions(); return }
        if name == "start" {
            guard let id = value["sessionId"] as? String, let locale = value["locale"] as? String else { return }
            start(id: id, locale: locale, onDeviceOnly: value["onDeviceOnly"] as? Bool ?? false)
            return
        }
        guard let id = value["sessionId"] as? String, id == sessionID else { return }
        switch name {
        case "stop": stop()
        case "cancel": cancel()
        case "copy": copyResult()
        case "insert": insertResult()
        default: break
        }
    }
    private func focusedElement(_ pid: pid_t) -> AXUIElement? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(pid), kAXFocusedUIElementAttribute as CFString, &value) == .success,
              let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return (value as! AXUIElement)
    }
    private func start(id: String, locale: String, onDeviceOnly: Bool) {
        cleanupAudio()
        sessionID = id; transcript = ""; finalizing = false; finished = false
        permissions()
        guard microphoneStatus() == "granted", speechStatus() == "granted" else { fail("VOICE_PERMISSION_REQUIRED"); return }
        guard let recognizer = SFSpeechRecognizer(locale: Locale(identifier: locale)), recognizer.isAvailable else {
            fail("VOICE_RECOGNIZER_UNAVAILABLE"); return
        }
        if onDeviceOnly && !recognizer.supportsOnDeviceRecognition { fail("VOICE_OFFLINE_UNAVAILABLE"); return }
        let application = NSWorkspace.shared.frontmostApplication
        targetPID = application?.processIdentifier
        targetElement = targetPID.flatMap { focusedElement($0) }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.requiresOnDeviceRecognition = onDeviceOnly
        request.addsPunctuation = true
        let engine = AVAudioEngine()
        let node = engine.inputNode
        let format = node.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else { fail("VOICE_MICROPHONE_UNAVAILABLE"); return }
        node.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in request.append(buffer) }
        self.engine = engine; self.request = request
        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            DispatchQueue.main.async {
                guard let self, self.sessionID == id, !self.finished else { return }
                if let result {
                    self.transcript = String(result.bestTranscription.formattedString.prefix(16_384))
                    if result.isFinal { self.complete(); return }
                    if Date().timeIntervalSince(self.lastPartialAt) >= 0.1 {
                        self.lastPartialAt = Date()
                        self.emit("partial", ["text": self.transcript])
                        self.showHUD(self.transcript.isEmpty ? "正在聆听…" : self.transcript)
                    }
                }
                if error != nil { self.fail("VOICE_RECOGNITION_FAILED") }
            }
        }
        do { engine.prepare(); try engine.start() }
        catch { fail("VOICE_RECORDING_FAILED"); return }
        showHUD("正在聆听…")
        deadline = Timer.scheduledTimer(withTimeInterval: 120, repeats: false) { [weak self] _ in self?.stop() }
    }
    private func stop() {
        guard sessionID != nil, !finished, !finalizing else { return }
        finalizing = true
        emit("finalizing")
        deadline?.invalidate()
        engine?.stop(); engine?.inputNode.removeTap(onBus: 0); engine = nil
        request?.endAudio()
        showHUD(transcript.isEmpty ? "正在转写…" : transcript)
        deadline = Timer.scheduledTimer(withTimeInterval: 12, repeats: false) { [weak self] _ in
            self?.fail("VOICE_FINALIZATION_TIMEOUT")
        }
    }
    private func complete() {
        guard !finished else { return }
        finished = true
        cleanupAudio()
        if transcript.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { fail("VOICE_NO_SPEECH"); return }
        emit("final", ["text": transcript])
        showHUD(transcript)
    }
    private func fail(_ code: String) {
        finished = true
        cleanupAudio()
        emit("error", ["code": code])
        showHUD("语音输入未完成，请在 Studio 查看详情。")
    }
    func shutdown() { cleanupAudio(); panel?.orderOut(nil); transcript = ""; sessionID = nil }
    private func cleanupAudio() {
        deadline?.invalidate(); deadline = nil
        if let engine { engine.stop(); engine.inputNode.removeTap(onBus: 0) }
        engine = nil
        request?.endAudio(); request = nil
        task?.cancel(); task = nil
    }
    @objc private func cancel() {
        finished = true
        cleanupAudio()
        emit("cancelled")
        sessionID = nil; transcript = ""; targetElement = nil; targetPID = nil
        panel?.orderOut(nil)
    }
    @objc private func copyResult() {
        guard finished, !transcript.isEmpty else { return }
        NSPasteboard.general.clearContents()
        let copied = NSPasteboard.general.setString(transcript, forType: .string)
        emit("output", ["result": copied ? "copied" : "copyFailed"])
    }
    private func insertResult() {
        guard finished, !transcript.isEmpty else { return }
        guard AXIsProcessTrusted() else { emit("output", ["result":"permissionRequired"]); return }
        guard let pid = targetPID, let element = targetElement,
              NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
              let current = focusedElement(pid), CFEqual(element, current) else {
            emit("output", ["result":"targetChanged"]); showHUD("输入位置已变化，请复制结果：\n" + transcript); return
        }
        // Never inject into password fields; retain the result for explicit copy.
        var subrole: CFTypeRef?
        _ = AXUIElementCopyAttributeValue(current, kAXSubroleAttribute as CFString, &subrole)
        if subrole as? String == kAXSecureTextFieldSubrole as String { emit("output", ["result":"secureField"]); return }
        guard let down = CGEvent(keyboardEventSource: nil, virtualKey: 9, keyDown: true),
              let up = CGEvent(keyboardEventSource: nil, virtualKey: 9, keyDown: false) else {
            emit("output", ["result":"insertFailed"]); return
        }
        let pasteboard = NSPasteboard.general
        let oldItems: [[NSPasteboard.PasteboardType: Data]] = (pasteboard.pasteboardItems ?? []).map { item in
            Dictionary(uniqueKeysWithValues: item.types.compactMap { type in item.data(forType: type).map { (type, $0) } })
        }
        func restoreClipboard() {
            let items = oldItems.map { fields -> NSPasteboardItem in
                let item = NSPasteboardItem(); for (type, data) in fields { item.setData(data, forType: type) }; return item
            }
            pasteboard.clearContents(); pasteboard.writeObjects(items)
        }
        pasteboard.clearContents()
        guard pasteboard.setString(transcript, forType: .string) else {
            restoreClipboard()
            emit("output", ["result":"insertFailed"]); return
        }
        let changeCount = pasteboard.changeCount
        down.flags = .maskCommand; up.flags = .maskCommand
        down.post(tap: .cghidEventTap); up.post(tap: .cghidEventTap)
        // Posting the event is observable; acceptance by another app is not.
        emit("output", ["result":"pasteSent"])
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            guard pasteboard.changeCount == changeCount else { return }
            restoreClipboard()
        }
    }
    private func showHUD(_ text: String) {
        if panel == nil {
            let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 440, height: 130),
                                styleMask: [.nonactivatingPanel, .titled], backing: .buffered, defer: false)
            panel.title = "AhaKey 语音输入"; panel.level = .floating; panel.hidesOnDeactivate = false
            panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            let label = NSTextField(wrappingLabelWithString: "")
            label.frame = NSRect(x: 16, y: 42, width: 408, height: 74); label.maximumNumberOfLines = 3
            panel.contentView?.addSubview(label)
            let copy = NSButton(title: "复制结果", target: self, action: #selector(copyResult))
            copy.frame = NSRect(x: 240, y: 8, width: 92, height: 28); panel.contentView?.addSubview(copy)
            let cancel = NSButton(title: "取消 / 清除", target: self, action: #selector(cancel))
            cancel.frame = NSRect(x: 336, y: 8, width: 92, height: 28); panel.contentView?.addSubview(cancel)
            if let screen = NSScreen.main?.visibleFrame {
                panel.setFrameOrigin(NSPoint(x: screen.midX - 220, y: screen.minY + 60))
            }
            self.panel = panel; self.label = label
        }
        label?.stringValue = text
        panel?.orderFrontRegardless()
    }
}

let application = NSApplication.shared
application.setActivationPolicy(.accessory)
let helper = SpeechHelper()
helper.permissions()
if CommandLine.arguments.contains("--status") { exit(0) }
DispatchQueue.global(qos: .userInitiated).async {
    while let line = readLine() {
        guard line.utf8.count <= 8192, let data = line.data(using: .utf8),
              let command = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
        DispatchQueue.main.async { helper.command(command) }
    }
    DispatchQueue.main.async { helper.shutdown(); application.terminate(nil) }
}
application.run()
