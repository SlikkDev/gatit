import AVFoundation
import Foundation

/// Records the default microphone (your side of the call) to its own file, so the
/// transcript can label "Me" and "Them" without any speaker detection.
final class MicrophoneRecorder {
    private let engine = AVAudioEngine()
    private let downmix = AVAudioMixerNode()
    private var file: AVAudioFile?
    private let levels: LevelMeter

    init(levels: LevelMeter) {
        self.levels = levels
    }

    func start(writingTo url: URL) throws {
        let input = engine.inputNode
        let inputFormat = input.outputFormat(forBus: 0)
        guard inputFormat.sampleRate > 0 else { throw RecorderError.message("no microphone available") }

        // Some interfaces expose many input channels; fold everything into mono.
        guard let mono = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: inputFormat.sampleRate,
                                       channels: 1, interleaved: false)
        else { throw RecorderError.message("cannot build mono format") }
        engine.attach(downmix)
        engine.connect(input, to: downmix, format: inputFormat)
        engine.connect(downmix, to: engine.mainMixerNode, format: mono)
        // The engine needs an output to pull audio through; keep it silent so the
        // microphone is never played back into the call.
        engine.mainMixerNode.outputVolume = 0

        let file = try AVAudioFile(forWriting: url, settings: aacSettings(for: mono),
                                   commonFormat: .pcmFormatFloat32, interleaved: false)
        self.file = file

        downmix.installTap(onBus: 0, bufferSize: 4096, format: mono) { [weak self] buffer, _ in
            guard let self else { return }
            do {
                try self.file?.write(from: buffer)
                self.levels.update(.mic, buffer: buffer)
            } catch {
                emit(["event": "error", "source": "mic", "message": "\(error)"])
            }
        }
        engine.prepare()
        try engine.start()
    }

    func stop() {
        downmix.removeTap(onBus: 0)
        engine.stop()
        file = nil
    }

    static func requestPermission() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: return true
        case .notDetermined: return await AVCaptureDevice.requestAccess(for: .audio)
        default: return false
        }
    }
}
