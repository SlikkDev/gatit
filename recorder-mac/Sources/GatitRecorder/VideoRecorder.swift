import AVFoundation
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

/// Records the call's video by capturing the call app's window (the Zoom meeting window, the
/// browser window showing Google Meet, or WhatsApp) with ScreenCaptureKit. Like the audio,
/// nothing joins the call; macOS asks once for "Screen & System Audio Recording".
///
/// The video is written on its own to screen.mp4, with screen.json saying how many seconds after
/// the audio it starts. `muxVideo(in:)` later combines it with both audio tracks into video.mp4.
/// If no call window is open yet, it keeps looking, and if the window closes (or Zoom swaps it
/// for another) it picks up the next one, continuing the same file.
final class VideoRecorder: NSObject, SCStreamOutput, SCStreamDelegate {
    private let url: URL
    private let audioStart: CMTime
    private let queue = DispatchQueue(label: "gatit.video", qos: .userInitiated)

    // Only touched on `queue`.
    private var stream: SCStream?
    private var writer: AVAssetWriter?
    private var input: AVAssetWriterInput?
    private var size: CGSize?
    private var firstFrame: CMTime?
    private var stopped = false

    private var searchTask: Task<Void, Never>?

    /// `audioStart` is the host-clock time the audio files start at, to line the video up with them.
    init(writingTo url: URL, audioStart: CMTime) {
        self.url = url
        self.audioStart = audioStart
    }

    func start() {
        guard CGPreflightScreenCaptureAccess() else {
            // Shows the system prompt the first time; once allowed, it applies from the next call.
            _ = CGRequestScreenCaptureAccess()
            emit(["event": "notice", "message": "Recording audio only. To also record video, allow Gatit in System Settings > Privacy & Security > Screen & System Audio Recording."])
            return
        }
        searchTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                if self.queue.sync(execute: { self.stream == nil && !self.stopped }) {
                    await self.attachToCallWindow()
                }
                try? await Task.sleep(nanoseconds: 3_000_000_000)
            }
        }
    }

    private func attachToCallWindow() async {
        guard let content = try? await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true),
              let match = Self.callWindow(in: content.windows)
        else { return }
        let (window, app) = match

        let config = SCStreamConfiguration()
        // The output size is fixed by the first window so later windows continue the same file.
        let size = queue.sync { () -> CGSize in
            if let size = self.size { return size }
            // Twice the window's point size covers Retina; cap it so an hour stays a few hundred MB.
            var width = min(1920, window.frame.width * 2)
            var height = width * window.frame.height / max(window.frame.width, 1)
            if height > 1200 { width *= 1200 / height; height = 1200 }
            let even = CGSize(width: (Int(width) / 2) * 2, height: (Int(height) / 2) * 2)
            self.size = even
            return even
        }
        config.width = Int(size.width)
        config.height = Int(size.height)
        config.scalesToFit = true
        config.preservesAspectRatio = true
        config.minimumFrameInterval = CMTime(value: 1, timescale: 15)
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        config.showsCursor = false
        config.queueDepth = 6

        let filter = SCContentFilter(desktopIndependentWindow: window)
        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        do {
            try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
            let attached = queue.sync { () -> Bool in
                guard !self.stopped else { return false }
                self.stream = stream
                return true
            }
            guard attached else { return }
            try await stream.startCapture()
            emit(["event": "video", "app": app, "window": window.title ?? ""])
        } catch {
            queue.sync { if self.stream === stream { self.stream = nil } }
            emit(["event": "notice", "message": "Couldn't record the call's video: \(error.localizedDescription)"])
        }
    }

    /// Picks the window most likely to be the call: a Zoom meeting, a Google Meet tab, or a
    /// WhatsApp window (preferring its call window). Ties go to the biggest window.
    static func callWindow(in windows: [SCWindow]) -> (SCWindow, String)? {
        let browsers = ["com.google.Chrome", "com.apple.Safari", "company.thebrowser.Browser",
                        "com.microsoft.edgemac", "com.brave.Browser", "org.mozilla.firefox",
                        "com.vivaldi.Vivaldi", "com.operasoftware.Opera"]
        var best: (window: SCWindow, app: String, score: Int)?
        for window in windows where window.windowLayer == 0 && window.frame.width >= 320 && window.frame.height >= 200 {
            guard let bundle = window.owningApplication?.bundleIdentifier else { continue }
            let title = window.title ?? ""
            let lower = title.lowercased()
            var app = ""
            var score = 0
            if bundle == "us.zoom.xos" {
                app = "Zoom"
                if lower.contains("zoom meeting") || lower.contains("webinar") { score = 3 }
            } else if browsers.contains(where: { bundle.hasPrefix($0) }) {
                app = "Google Meet"
                if lower.hasPrefix("meet -") || lower.hasPrefix("meet –") || lower.contains("google meet") { score = 3 }
            } else if bundle.hasPrefix("net.whatsapp.WhatsApp") || bundle == "desktop.WhatsApp" {
                app = "WhatsApp"
                score = (lower.contains("call") || title.contains("שיחה")) ? 3 : 1
            }
            guard score > 0 else { continue }
            let area = window.frame.width * window.frame.height
            if let current = best {
                let currentArea = current.window.frame.width * current.window.frame.height
                if score < current.score || (score == current.score && area <= currentArea) { continue }
            }
            best = (window, app, score)
        }
        return best.map { ($0.window, $0.app) }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, !stopped, sampleBuffer.isValid,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
                as? [[SCStreamFrameInfo: Any]],
              let rawStatus = attachments.first?[.status] as? Int,
              SCFrameStatus(rawValue: rawStatus) == .complete
        else { return }
        let time = sampleBuffer.presentationTimeStamp

        if writer == nil {
            guard let size else { return }
            do {
                try? FileManager.default.removeItem(at: url)
                let writer = try AVAssetWriter(outputURL: url, fileType: .mp4)
                let input = AVAssetWriterInput(mediaType: .video, outputSettings: [
                    AVVideoCodecKey: AVVideoCodecType.h264,
                    AVVideoWidthKey: Int(size.width),
                    AVVideoHeightKey: Int(size.height),
                    AVVideoCompressionPropertiesKey: [
                        AVVideoAverageBitRateKey: 1_200_000,
                        AVVideoExpectedSourceFrameRateKey: 15,
                        AVVideoMaxKeyFrameIntervalDurationKey: 4,
                        AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
                    ],
                ])
                input.expectsMediaDataInRealTime = true
                writer.add(input)
                guard writer.startWriting() else { throw writer.error ?? RecorderError.message("can't start video file") }
                writer.startSession(atSourceTime: time)
                self.writer = writer
                self.input = input
                firstFrame = time
            } catch {
                emit(["event": "notice", "message": "Couldn't record the call's video: \(error)"])
                stopped = true
                return
            }
        }
        if let input, input.isReadyForMoreMediaData, writer?.status == .writing {
            input.append(sampleBuffer)
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        // Usually the call window closed. Look for the next one.
        queue.async { if self.stream === stream { self.stream = nil } }
    }

    /// Stops capturing and finishes screen.mp4. Blocks until the file is closed.
    func stop() {
        searchTask?.cancel()
        let current = queue.sync { () -> SCStream? in
            stopped = true
            defer { stream = nil }
            return stream
        }
        let done = DispatchSemaphore(value: 0)
        if let current {
            current.stopCapture { _ in done.signal() }
            _ = done.wait(timeout: .now() + 3)
        }
        // Wait for any frame still being appended, then close the file.
        let (writer, input, firstFrame) = queue.sync { (self.writer, self.input, self.firstFrame) }
        guard let writer, let input, let firstFrame, writer.status == .writing else { return }
        input.markAsFinished()
        writer.endSession(atSourceTime: CMClockGetTime(CMClockGetHostTimeClock()))
        writer.finishWriting { done.signal() }
        _ = done.wait(timeout: .now() + 15)

        let offset = max(0, CMTimeGetSeconds(CMTimeSubtract(firstFrame, audioStart)))
        if let data = try? JSONSerialization.data(withJSONObject: ["offset": offset]) {
            try? data.write(to: url.deletingLastPathComponent().appendingPathComponent("screen.json"))
        }
    }
}

/// Turns screen.mp4 plus mic.m4a and system.m4a into video.mp4: the call's video with both sides
/// of the conversation mixed into one audio track, lined up, playable anywhere. The video is
/// copied, not re-encoded, so this takes seconds. screen.mp4 is removed once video.mp4 exists.
func muxVideo(in folder: URL) async throws {
    let files = FileManager.default
    let screen = folder.appendingPathComponent("screen.mp4")
    let screenInfo = folder.appendingPathComponent("screen.json")
    let mixed = folder.appendingPathComponent("video-audio.partial.m4a")
    let partial = folder.appendingPathComponent("video.partial.mp4")
    let output = folder.appendingPathComponent("video.mp4")
    defer {
        try? files.removeItem(at: mixed)
        try? files.removeItem(at: partial)
    }

    let info = (try? Data(contentsOf: screenInfo))
        .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
    let offset = info?["offset"] as? Double ?? 0

    // 1. Mix "Me" and "Them" into one track (an export mixes every audio track it is given).
    let voices = AVMutableComposition()
    for name in ["system.m4a", "mic.m4a"] {
        let url = folder.appendingPathComponent(name)
        guard files.fileExists(atPath: url.path) else { continue }
        let asset = AVURLAsset(url: url)
        guard let track = try await asset.loadTracks(withMediaType: .audio).first else { continue }
        let duration = try await asset.load(.duration)
        let target = voices.addMutableTrack(withMediaType: .audio, preferredTrackID: kCMPersistentTrackID_Invalid)
        try target?.insertTimeRange(CMTimeRange(start: .zero, duration: duration), of: track, at: .zero)
    }
    let hasAudio = !voices.tracks.isEmpty
    if hasAudio {
        try? files.removeItem(at: mixed)
        try await export(voices, preset: AVAssetExportPresetAppleM4A, to: mixed, as: .m4a)
    }

    // 2. Put the video where it started relative to the audio, and copy both into video.mp4.
    let movie = AVMutableComposition()
    let screenAsset = AVURLAsset(url: screen)
    guard let videoTrack = try await screenAsset.loadTracks(withMediaType: .video).first else {
        throw RecorderError.message("screen.mp4 has no video")
    }
    let videoDuration = try await screenAsset.load(.duration)
    let video = movie.addMutableTrack(withMediaType: .video, preferredTrackID: kCMPersistentTrackID_Invalid)
    try video?.insertTimeRange(CMTimeRange(start: .zero, duration: videoDuration), of: videoTrack,
                               at: CMTime(seconds: offset, preferredTimescale: 600))
    if hasAudio {
        let audioAsset = AVURLAsset(url: mixed)
        if let audioTrack = try await audioAsset.loadTracks(withMediaType: .audio).first {
            let duration = try await audioAsset.load(.duration)
            let audio = movie.addMutableTrack(withMediaType: .audio, preferredTrackID: kCMPersistentTrackID_Invalid)
            try audio?.insertTimeRange(CMTimeRange(start: .zero, duration: duration), of: audioTrack, at: .zero)
        }
    }
    try? files.removeItem(at: partial)
    try await export(movie, preset: AVAssetExportPresetPassthrough, to: partial, as: .mp4)
    try? files.removeItem(at: output)
    try files.moveItem(at: partial, to: output)
    try? files.removeItem(at: screen)
    try? files.removeItem(at: screenInfo)
}

private func export(_ asset: AVAsset, preset: String, to url: URL, as type: AVFileType) async throws {
    guard let session = AVAssetExportSession(asset: asset, presetName: preset) else {
        throw RecorderError.message("can't export with \(preset)")
    }
    session.outputURL = url
    session.outputFileType = type
    await session.export()
    guard session.status == .completed else {
        throw RecorderError.message("export failed: \(session.error.map { "\($0)" } ?? "unknown error")")
    }
}
