import Foundation

// gatit-recorder --out <folder>
//
// Records the microphone to <folder>/mic.m4a and system audio to <folder>/system.m4a
// until it reads "stop" on stdin or receives SIGINT/SIGTERM. Progress is reported to
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
guard let outIndex = arguments.firstIndex(of: "--out"), outIndex + 1 < arguments.count else {
    fail("usage: gatit-recorder --out <folder>")
}
let folder = URL(fileURLWithPath: arguments[outIndex + 1], isDirectory: true)

let levels = LevelMeter()
let mic = MicrophoneRecorder(levels: levels)
let system = SystemAudioRecorder(levels: levels)
var stopped = false
let stopLock = NSLock()

func stopAndExit() {
    stopLock.lock()
    defer { stopLock.unlock() }
    guard !stopped else { return }
    stopped = true
    mic.stop()
    system.stop()
    emit(["event": "stopped", "folder": folder.path])
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

// Stop on "stop" from stdin, or when the parent app goes away and stdin closes.
Thread.detachNewThread {
    while let line = readLine() {
        if line.trimmingCharacters(in: .whitespaces) == "stop" { break }
    }
    DispatchQueue.main.async { stopAndExit() }
}

dispatchMain()
