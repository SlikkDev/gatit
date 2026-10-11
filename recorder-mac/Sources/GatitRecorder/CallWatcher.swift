import AppKit
import CoreAudio
import Foundation

// gatit-recorder --watch-calls
//
// Watches which apps are using the microphone, the same signal behind the orange dot in the
// menu bar, and reports when a call app (Zoom, WhatsApp, a browser running Meet, ...) starts
// or stops using it:
//
//   {"event":"call_started","app":"Zoom","source":"Zoom"}
//   {"event":"call_ended","app":"Zoom"}
//
// It only reads Core Audio's list of audio clients, so it needs no permission and never
// touches any audio. It exits when stdin closes (the Gatit app went away).

/// A call app, matched by bundle identifier prefix (lowercased). `source` is the value
/// stored as the meeting's app: browsers are assumed to be running Google Meet.
private let callApps: [(prefix: String, name: String, source: String)] = [
    ("us.zoom.", "Zoom", "Zoom"),
    ("net.whatsapp.", "WhatsApp", "WhatsApp"),
    ("desktop.whatsapp", "WhatsApp", "WhatsApp"),
    ("com.google.chrome", "Google Chrome", "Meet"),
    ("com.apple.safari", "Safari", "Meet"),
    ("com.apple.webkit.gpu", "Safari", "Meet"),
    ("company.thebrowser.", "Arc", "Meet"),
    ("com.microsoft.edgemac", "Microsoft Edge", "Meet"),
    ("com.brave.browser", "Brave", "Meet"),
    ("org.mozilla.firefox", "Firefox", "Meet"),
    ("com.vivaldi.", "Vivaldi", "Meet"),
    ("com.operasoftware.", "Opera", "Meet"),
    ("com.microsoft.teams", "Microsoft Teams", "Other"),
]

/// Gatit's own recorder uses the mic too; never treat that as a call.
private let ownBundlePrefix = "dev.slikk.gatit"

private func property<T>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, _ initial: T) -> T? {
    var address = AudioObjectPropertyAddress(mSelector: selector,
                                             mScope: kAudioObjectPropertyScopeGlobal,
                                             mElement: kAudioObjectPropertyElementMain)
    var value = initial
    var size = UInt32(MemoryLayout<T>.size)
    let status = withUnsafeMutablePointer(to: &value) {
        AudioObjectGetPropertyData(object, &address, 0, nil, &size, $0)
    }
    return status == noErr ? value : nil
}

private func audioProcessObjects() -> [AudioObjectID]? {
    var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyProcessObjectList,
                                             mScope: kAudioObjectPropertyScopeGlobal,
                                             mElement: kAudioObjectPropertyElementMain)
    let system = AudioObjectID(kAudioObjectSystemObject)
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(system, &address, 0, nil, &size) == noErr else { return nil }
    var ids = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
    guard AudioObjectGetPropertyData(system, &address, 0, nil, &size, &ids) == noErr else { return nil }
    return ids
}

private func bundleID(ofAudioProcess object: AudioObjectID) -> String? {
    var address = AudioObjectPropertyAddress(mSelector: kAudioProcessPropertyBundleID,
                                             mScope: kAudioObjectPropertyScopeGlobal,
                                             mElement: kAudioObjectPropertyElementMain)
    var value: Unmanaged<CFString>?
    var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    guard AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr,
          let cfID = value?.takeRetainedValue()
    else { return nil }
    let id = cfID as String
    return id.isEmpty ? nil : id
}

private func parentPID(of pid: pid_t) -> pid_t? {
    var info = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.stride
    var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
    guard sysctl(&mib, 4, &info, &size, nil, 0) == 0, size > 0 else { return nil }
    let parent = info.kp_eproc.e_ppid
    return parent > 1 ? parent : nil
}

/// Bundle identifiers for a process and the apps it descends from. Browsers capture audio in
/// helper processes, so the helper's own identifier may be unknown or generic.
private func bundleIDs(ofAudioProcess object: AudioObjectID) -> [String] {
    var ids: [String] = []
    if let id = bundleID(ofAudioProcess: object) { ids.append(id) }
    var pid: pid_t? = property(object, kAudioProcessPropertyPID, pid_t(0))
    var depth = 0
    while let current = pid, current > 1, depth < 8 {
        if let id = NSRunningApplication(processIdentifier: current)?.bundleIdentifier {
            ids.append(id)
        }
        pid = parentPID(of: current)
        depth += 1
    }
    return ids.map { $0.lowercased() }
}

private func callApp(for ids: [String]) -> (name: String, source: String)? {
    if ids.contains(where: { $0.hasPrefix(ownBundlePrefix) }) { return nil }
    for id in ids {
        if let match = callApps.first(where: { id.hasPrefix($0.prefix) }) {
            return (match.name, match.source)
        }
    }
    return nil
}

/// Names of the call apps using the microphone right now.
private func appsUsingMicrophone() -> [String: String]? {
    guard let objects = audioProcessObjects() else { return nil }
    var apps: [String: String] = [:]
    for object in objects {
        guard let running: UInt32 = property(object, kAudioProcessPropertyIsRunningInput, UInt32(0)),
              running != 0,
              let app = callApp(for: bundleIDs(ofAudioProcess: object))
        else { continue }
        apps[app.name] = app.source
    }
    return apps
}

func runCallWatcher() -> Never {
    // Debounce: a call has started once the mic has been in use for 2 seconds (so a quick
    // audio check doesn't count), and has ended after 4 seconds without it (so switching
    // microphones mid-call doesn't count as a new call).
    let startAfter = 2
    let endAfter = 4
    var seen: [String: Int] = [:]
    var missing: [String: Int] = [:]
    var active: Set<String> = []
    var failures = 0

    let timer = DispatchSource.makeTimerSource(queue: .main)
    timer.schedule(deadline: .now(), repeating: 1.0)
    timer.setEventHandler {
        guard let apps = appsUsingMicrophone() else {
            failures += 1
            if failures == 5 { emit(["event": "error", "message": "can't read which apps use the microphone"]) }
            return
        }
        failures = 0
        for (name, source) in apps {
            missing[name] = 0
            seen[name, default: 0] += 1
            if seen[name]! >= startAfter, !active.contains(name) {
                active.insert(name)
                emit(["event": "call_started", "app": name, "source": source])
            }
        }
        for name in Set(seen.keys).subtracting(apps.keys) {
            missing[name, default: 0] += 1
            if missing[name]! >= endAfter {
                seen[name] = nil
                missing[name] = nil
                if active.remove(name) != nil {
                    emit(["event": "call_ended", "app": name])
                }
            }
        }
    }
    timer.resume()
    emit(["event": "watching"])

    Thread.detachNewThread {
        while readLine() != nil {}
        exit(0)
    }
    RunLoop.main.run()
    exit(0)
}
