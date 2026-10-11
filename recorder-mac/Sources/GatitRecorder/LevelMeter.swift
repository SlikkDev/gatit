import AVFoundation
import Foundation

/// Tracks peak loudness per source so the app can show that both sides are being heard.
final class LevelMeter {
    enum Source { case mic, system }

    private let lock = NSLock()
    private var peaks: [Source: Float] = [.mic: 0, .system: 0]

    func update(_ source: Source, buffer: AVAudioPCMBuffer) {
        guard let channels = buffer.floatChannelData else { return }
        let frames = Int(buffer.frameLength)
        let stride = buffer.stride
        let channelCount = buffer.format.isInterleaved ? 1 : Int(buffer.format.channelCount)
        var sum: Float = 0
        var count = 0
        for channel in 0..<channelCount {
            let samples = channels[channel]
            for frame in Swift.stride(from: 0, to: frames * stride, by: stride) {
                sum += samples[frame] * samples[frame]
                count += 1
            }
        }
        let rms = count > 0 ? (sum / Float(count)).squareRoot() : 0
        lock.lock()
        peaks[source] = max(peaks[source] ?? 0, rms)
        lock.unlock()
    }

    /// Returns the loudest level seen since the last call, then resets.
    func drain() -> (mic: Float, system: Float) {
        lock.lock()
        defer {
            peaks = [.mic: 0, .system: 0]
            lock.unlock()
        }
        return (peaks[.mic] ?? 0, peaks[.system] ?? 0)
    }
}
