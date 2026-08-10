#[cfg(target_os = "macos")]
pub fn is_enabled() -> bool {
    plist_path().is_some_and(|path| path.exists())
}

#[cfg(target_os = "macos")]
pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let path = plist_path().ok_or_else(|| anyhow::anyhow!("home directory unavailable"))?;
    if enabled {
        let executable = xml_escape(&std::env::current_exe()?.display().to_string());
        let contents = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>dev.epd-kit.agent</string>
<key>ProgramArguments</key><array><string>{executable}</string><string>--no-open</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
</dict></plist>"#
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn plist_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|home| home.join("Library/LaunchAgents/dev.epd-kit.agent.plist"))
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(target_os = "windows")]
pub fn is_enabled() -> bool {
    std::process::Command::new("reg")
        .args(["query", RUN_KEY, "/v", "EPD Agent"])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "windows")]
pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    use std::process::Command;
    if enabled {
        let executable = std::env::current_exe()?;
        let value = format!("\"{}\" --no-open", executable.display());
        let status = Command::new("reg")
            .args([
                "add",
                RUN_KEY,
                "/v",
                "EPD Agent",
                "/t",
                "REG_SZ",
                "/d",
                &value,
                "/f",
            ])
            .status()?;
        anyhow::ensure!(status.success(), "failed to enable Windows autostart");
    } else {
        let status = Command::new("reg")
            .args(["delete", RUN_KEY, "/v", "EPD Agent", "/f"])
            .status()?;
        anyhow::ensure!(
            status.success() || !is_enabled(),
            "failed to disable Windows autostart"
        );
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn set_enabled(_enabled: bool) -> anyhow::Result<()> {
    anyhow::bail!("autostart is supported on macOS and Windows")
}
