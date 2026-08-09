//! Lightweight desktop notifications for price alerts.
//!
//! The app remains usable without a notification service. Platform commands
//! are launched off the UI thread and failures are deliberately ignored; the
//! in-app status line still records that an alert fired.

/// Send a best-effort desktop notification.
pub fn send(title: String, body: String) {
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                escape_applescript(&body),
                escape_applescript(&title)
            );
            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .status();
        }

        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("notify-send")
                .arg("-a")
                .arg("ZStock")
                .arg(&title)
                .arg(&body)
                .status();
        }

        // Windows support can be added with a native toast backend later;
        // keeping this a no-op is preferable to blocking quote polling.
        #[cfg(target_os = "windows")]
        {
            let _ = (title, body);
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = (title, body);
        }
    });
}

#[cfg(target_os = "macos")]
fn escape_applescript(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n'], " ")
}
