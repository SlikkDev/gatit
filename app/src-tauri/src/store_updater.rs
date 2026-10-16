//! Microsoft Store owns installation and updates of the MSIX edition.
//! Keep this separate so Store builds cannot download or launch the NSIS installer.
use tauri::menu::Menu;
use tauri::{AppHandle, Wry};

pub fn ready_version() -> Option<String> {
    None
}

pub fn start(_app: &AppHandle, _menu: Menu<Wry>) {}

pub fn restart(_app: &AppHandle) -> Result<(), String> {
    Err("Updates for this edition are managed by Microsoft Store.".into())
}

#[cfg(windows)]
pub fn check_runtime() -> bool {
    if tauri::webview_version().is_ok() {
        return true;
    }
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONINFORMATION, MB_YESNO};
    let text: Vec<u16> = "Gatit needs the Microsoft Edge WebView2 Runtime. Open Microsoft's download page to install it, then reopen Gatit?\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "Gatit\0".encode_utf16().collect();
    // Both UTF-16 buffers remain alive and null-terminated throughout this call.
    let choice = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONINFORMATION,
        )
    };
    if choice == IDYES {
        let _ = open::that("https://developer.microsoft.com/microsoft-edge/webview2/");
    }
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn store_never_reports_a_direct_download_update() {
        assert_eq!(super::ready_version(), None);
    }
}
