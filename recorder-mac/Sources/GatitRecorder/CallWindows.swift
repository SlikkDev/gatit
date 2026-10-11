import CoreGraphics
import Foundation

/// The windows of the apps calls happen in, reported once when recording starts so Gatit can
/// tell which app the call is in and, from some titles, who is on it: Zoom titles a meeting
/// "<Host>'s Zoom Meeting", WhatsApp titles a call window after the contact.
///
/// macOS only reveals window titles to apps with Screen Recording permission. Without it the
/// list still names the apps, with empty titles, and Gatit falls back to guessing nothing.
/// Asking for the list never prompts for the permission.
enum CallWindows {
    static let apps: Set<String> = [
        "zoom.us", "WhatsApp",
        "Google Chrome", "Safari", "Arc", "Firefox", "Microsoft Edge", "Brave Browser",
        "Chromium", "Vivaldi", "Opera",
    ]

    static func list() -> [[String: String]] {
        let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
        guard let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
            return []
        }
        return windows.compactMap { info in
            guard let owner = info[kCGWindowOwnerName as String] as? String, apps.contains(owner),
                  // Layer 0 is ordinary windows; menu bar extras and overlays sit above it.
                  (info[kCGWindowLayer as String] as? Int) == 0 else { return nil }
            return ["app": owner, "title": info[kCGWindowName as String] as? String ?? ""]
        }
    }
}
