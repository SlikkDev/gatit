import AVFoundation
import CoreAudio
import Foundation

/// Records everything the Mac plays (Zoom, Meet in a browser, WhatsApp, ...) using a
/// Core Audio process tap. No bot joins the call and no screen-recording permission is
/// needed; macOS asks once for "System Audio Recording" instead.
final class SystemAudioRecorder {
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private var file: AVAudioFile?
    private let queue = DispatchQueue(label: "gatit.system-audio", qos: .userInitiated)
    private let levels: LevelMeter

    init(levels: LevelMeter) {
        self.levels = levels
    }

    func start(writingTo url: URL) throws {
        // A global tap that excludes nothing captures all system output. Our own
        // process plays no audio, so there is nothing to exclude.
        let tapDescription = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
        tapDescription.uuid = UUID()
        tapDescription.name = "Gatit system audio"
        tapDescription.isPrivate = true
        tapDescription.muteBehavior = .unmuted

        try check(AudioHardwareCreateProcessTap(tapDescription, &tapID), "create process tap")

        let outputUID = try defaultOutputDeviceUID()
        let aggregate: [String: Any] = [
            kAudioAggregateDeviceNameKey: "Gatit Tap",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceMainSubDeviceKey: outputUID,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outputUID]],
            kAudioAggregateDeviceTapListKey: [[
                kAudioSubTapDriftCompensationKey: true,
                kAudioSubTapUIDKey: tapDescription.uuid.uuidString,
            ]],
        ]
        try check(AudioHardwareCreateAggregateDevice(aggregate as CFDictionary, &aggregateID),
                  "create aggregate device")

        var streamDescription = try tapFormat()
        guard let format = AVAudioFormat(streamDescription: &streamDescription) else {
            throw RecorderError.message("unsupported system audio format")
        }
        let file = try AVAudioFile(forWriting: url,
                                   settings: aacSettings(for: format),
                                   commonFormat: .pcmFormatFloat32,
                                   interleaved: format.isInterleaved)
        self.file = file

        try check(AudioDeviceCreateIOProcIDWithBlock(&procID, aggregateID, queue) {
            [weak self] _, inputData, _, _, _ in
            guard let self,
                  let buffer = AVAudioPCMBuffer(pcmFormat: format, bufferListNoCopy: inputData, deallocator: nil)
            else { return }
            do {
                try self.file?.write(from: buffer)
                self.levels.update(.system, buffer: buffer)
            } catch {
                emit(["event": "error", "source": "system", "message": "\(error)"])
            }
        }, "create IO proc")

        try check(AudioDeviceStart(aggregateID, procID), "start aggregate device")
    }

    func stop() {
        if aggregateID != kAudioObjectUnknown {
            AudioDeviceStop(aggregateID, procID)
            if let procID { AudioDeviceDestroyIOProcID(aggregateID, procID) }
            AudioHardwareDestroyAggregateDevice(aggregateID)
            aggregateID = AudioObjectID(kAudioObjectUnknown)
        }
        if tapID != kAudioObjectUnknown {
            AudioHardwareDestroyProcessTap(tapID)
            tapID = AudioObjectID(kAudioObjectUnknown)
        }
        // Wait for any in-flight IO block before closing the file.
        queue.sync { file = nil }
    }

    private func tapFormat() throws -> AudioStreamBasicDescription {
        var address = AudioObjectPropertyAddress(mSelector: kAudioTapPropertyFormat,
                                                 mScope: kAudioObjectPropertyScopeGlobal,
                                                 mElement: kAudioObjectPropertyElementMain)
        var description = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        try check(AudioObjectGetPropertyData(tapID, &address, 0, nil, &size, &description), "read tap format")
        return description
    }

    private func defaultOutputDeviceUID() throws -> String {
        var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDefaultSystemOutputDevice,
                                                 mScope: kAudioObjectPropertyScopeGlobal,
                                                 mElement: kAudioObjectPropertyElementMain)
        var deviceID = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        try check(AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &deviceID),
                  "read default output device")

        address.mSelector = kAudioDevicePropertyDeviceUID
        var uid: Unmanaged<CFString>?
        size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        try check(AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, &uid), "read output device UID")
        guard let uid else { throw RecorderError.message("output device has no UID") }
        return uid.takeRetainedValue() as String
    }
}

func check(_ status: OSStatus, _ what: String) throws {
    guard status == noErr else { throw RecorderError.osStatus(what, status) }
}

/// AAC keeps an hour of call audio around 30 MB per track instead of ~1.4 GB of raw PCM.
func aacSettings(for format: AVAudioFormat) -> [String: Any] {
    [
        AVFormatIDKey: kAudioFormatMPEG4AAC,
        AVSampleRateKey: format.sampleRate,
        AVNumberOfChannelsKey: format.channelCount,
        AVEncoderBitRateKey: 64_000 * Int(min(format.channelCount, 2)),
    ]
}

enum RecorderError: Error, CustomStringConvertible {
    case message(String)
    case osStatus(String, OSStatus)

    var description: String {
        switch self {
        case .message(let text): return text
        case .osStatus(let what, let status): return "\(what) failed (OSStatus \(status))"
        }
    }
}
