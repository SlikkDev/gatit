import AppKit
import Foundation

// gatit-recorder --out <folder>
// gatit-recorder --watch-calls   (see CallWatcher.swift)
//
// Records the microphone to <folder>/mic.m4a and system audio to <folder>/system.m4a
// until it reads "stop" on stdin, receives SIGINT/SIGTERM, or the Mac is about to sleep. Progress is reported to
// stdout as one JSON object per line so the Gatit app can follow along.

func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object),
          let line = String(data: data, encoding: .utf8) else { return }
    FileHandle.standardOutput.write((line + "\n").data(using: .utf8)!)
}

func fail(_ message: String) -> Never {
    emit(["event": "error", "message": message])
    exit(1)
}

let arguments = CommandLine.arguments
if arguments.contains("--watch-calls") {
    runCallWatcher()
}
guard let outIndex = arguments.firstIndex(of: "--out"), outIndex + 1 < arguments.count else {
    fail("usage: gatit-recorder --out <folder>")
}
let folder = URL(fileURLWithPath: arguments[outIndex + 1], isDirectory: true)

let levels = LevelMeter()
let mic = MicrophoneRecorder(levels: levels)
let system = SystemAudioRecorder(levels: levels)
var stopped = false
let stopLock = NSLock()

func stopAndExit(reason: String = "user") {
    stopLock.lock()
    defer { stopLock.unlock() }
    guard !stopped else { return }
    stopped = true
    // Stopping closes both files properly; an .m4a that is never closed can't be played.
    mic.stop()
    system.stop()
    emit(["event": "stopped", "folder": folder.path, "reason": reason])
    exit(0)
}

Task {
    do {
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        guard await MicrophoneRecorder.requestPermission() else {
            fail("microphone permission denied; allow Gatit in System Settings > Privacy & Security > Microphone")
        }
        try system.start(writingTo: folder.appendingPathComponent("system.m4a"))
        try mic.start(writingTo: folder.appendingPathComponent("mic.m4a"))
        emit(["event": "started", "folder": folder.path])
        // Which app the call is in, and who is on it, as far as the open windows tell.
        emit(["event": "windows", "windows": CallWindows.list()])
    } catch {
        mic.stop()
        system.stop()
        fail("\(error)")
    }
}

// Report levels twice a second.
let levelTimer = DispatchSource.makeTimerSource(queue: .global())
levelTimer.schedule(deadline: .now() + 0.5, repeating: 0.5)
levelTimer.setEventHandler {
    let (micLevel, systemLevel) = levels.drain()
    emit(["event": "levels", "mic": Double(micLevel), "system": Double(systemLevel)])
}
levelTimer.resume()

// Stop on SIGINT/SIGTERM.
var signalSources: [DispatchSourceSignal] = []
for sig in [SIGINT, SIGTERM] {
    signal(sig, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
    source.setEventHandler { stopAndExit() }
    source.resume()
    signalSources.append(source)
}

// Finish both files before the Mac sleeps, so a call left recording overnight is still
// saved up to that point instead of being cut off mid-write.
let sleepObserver = NSWorkspace.shared.notificationCenter.addObserver(
    forName: NSWorkspace.willSleepNotification, object: nil, queue: .main
) { _ in stopAndExit(reason: "sleep") }

// Stop on "stop" from stdin, or when the parent app goes away and stdin closes.
Thread.detachNewThread {
    while let line = readLine() {
        if line.trimmingCharacters(in: .whitespaces) == "stop" { break }
    }
    DispatchQueue.main.async { stopAndExit() }
}

// The main run loop (rather than dispatchMain) also delivers the sleep notification.
RunLoop.main.run()
