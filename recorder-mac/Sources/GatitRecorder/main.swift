import AppKit
import CoreMedia
import Foundation

// gatit-recorder --out <folder> [--video]
// gatit-recorder --mux <folder>
//
// Records the microphone to <folder>/mic.m4a and system audio to <folder>/system.m4a
// (and with --video, the call app's window to <folder>/screen.mp4) until it reads "stop" on stdin, receives SIGINT/SIGTERM, or the Mac is about to sleep. Progress is reported to
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

// --mux: combine a finished recording's screen.mp4 and audio into video.mp4, then exit.
if let muxIndex = arguments.firstIndex(of: "--mux"), muxIndex + 1 < arguments.count {
    let folder = URL(fileURLWithPath: arguments[muxIndex + 1], isDirectory: true)
    Task {
        do {
            try await muxVideo(in: folder)
            emit(["event": "muxed", "folder": folder.path])
            exit(0)
        } catch {
            fail("\(error)")
        }
    }
    dispatchMain()
}

guard let outIndex = arguments.firstIndex(of: "--out"), outIndex + 1 < arguments.count else {
    fail("usage: gatit-recorder --out <folder>")
}
let folder = URL(fileURLWithPath: arguments[outIndex + 1], isDirectory: true)

let levels = LevelMeter()
let mic = MicrophoneRecorder(levels: levels)
let system = SystemAudioRecorder(levels: levels)
var video: VideoRecorder?
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
    video?.stop()
    emit(["event": "stopped", "folder": folder.path, "reason": reason])
    exit(0)
}

Task {
    do {
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        guard await MicrophoneRecorder.requestPermission() else {
            fail("microphone permission denied; allow Gatit in System Settings > Privacy & Security > Microphone")
        }
        let audioStart = CMClockGetTime(CMClockGetHostTimeClock())
        try system.start(writingTo: folder.appendingPathComponent("system.m4a"))
        try mic.start(writingTo: folder.appendingPathComponent("mic.m4a"))
        emit(["event": "started", "folder": folder.path])
        if arguments.contains("--video") {
            stopLock.withLock {
                guard !stopped else { return }
                let recorder = VideoRecorder(writingTo: folder.appendingPathComponent("screen.mp4"), audioStart: audioStart)
                video = recorder
                recorder.start()
            }
        }
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
