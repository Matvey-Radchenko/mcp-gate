//! Per-user service registration. Never installs a system service or kills a client.
use super::store::Record;
use anyhow::Result;
#[cfg(target_os = "linux")]
pub(super) mod linux;
#[cfg(any(target_os = "macos", windows))]
use anyhow::{Context, ensure};
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(any(target_os = "macos", windows))]
use std::process::Command;

#[cfg(target_os = "macos")]
fn agent_path(record: &Record) -> Result<PathBuf> {
    Ok(super::store::home()?
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", record.label)))
}
#[cfg(target_os = "macos")]
fn domain() -> String {
    // SAFETY: geteuid has no pointer arguments or preconditions.
    format!("gui/{}", unsafe { libc::geteuid() })
}
#[cfg(any(target_os = "macos", windows))]
fn run(command: &mut Command) -> Result<()> {
    let output = command.output().context("Cannot run service manager")?;
    ensure!(
        output.status.success(),
        // These commands contain only owned service names/file paths, never
        // backend arguments, environment values or authentication credentials.
        "Service manager failed ({}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim(),
        String::from_utf8_lossy(&output.stdout).trim()
    );
    Ok(())
}

/// Only service state/error codes are returned; command lines and environments
/// from the service manager's verbose response never reach user diagnostics.
#[cfg(target_os = "linux")]
pub fn diagnostics(record: &Record) -> String {
    linux::diagnostics(record)
}

#[cfg(not(target_os = "linux"))]
pub fn diagnostics(record: &Record) -> String {
    #[cfg(target_os = "macos")]
    if let Ok(output) = Command::new("launchctl")
        .args(["print", &format!("{}/{}", domain(), record.label)])
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let fields: Vec<_> = text
            .lines()
            .map(str::trim)
            .filter(|line| {
                line.starts_with("state =")
                    || line.starts_with("last exit code =")
                    || line.starts_with("last terminating signal =")
                    || line.starts_with("runs =")
            })
            .collect();
        if !fields.is_empty() {
            return fields.join("; ");
        }
    }
    if installed(record).unwrap_or(false) {
        "service registered; runtime health unavailable".into()
    } else {
        "service is not registered".into()
    }
}
pub fn register(record: &Record) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let path = agent_path(record)?;
        std::fs::create_dir_all(path.parent().context("Missing LaunchAgents directory")?)?;
        let binary = &record.binary;
        let xml = crate::install::xml;
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>Label</key><string>{}</string><key>ProgramArguments</key><array><string>{}</string><string>serve</string><string>--config</string><string>{}</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>ExitTimeOut</key><integer>20</integer><key>Umask</key><integer>63</integer>
<key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string></dict></plist>"#,
            xml(&record.label),
            xml(&binary.to_string_lossy()),
            xml(&record.config().to_string_lossy()),
            xml(&record.release.join("logs/stdout.log").to_string_lossy()),
            xml(&record.release.join("logs/stderr.log").to_string_lossy())
        );
        // create_new refuses to replace a label another setup already claimed.
        write_new(&path, plist.as_bytes(), &record.label)?;
        if let Err(error) = run(Command::new("launchctl")
            .args(["bootstrap", &domain()])
            .arg(&path))
        {
            let _ = std::fs::remove_file(&path);
            if label_in_use(&error) {
                return Err(label_taken(&record.label));
            }
            return Err(error);
        }
    }
    #[cfg(windows)]
    {
        let task = record.release.join("task.xml");
        // schtasks imports task definitions as Unicode XML. Match both its
        // encoding declaration and byte-order marker, including non-ASCII paths.
        let bytes: Vec<u8> = std::iter::once(0xfeff)
            .chain(windows_xml(record)?.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        super::store::atomic(&task, &bytes)?;
        if let Err(error) = run(Command::new("schtasks.exe")
            .args(["/Create", "/TN", &record.label, "/XML"])
            .arg(&task))
        {
            if label_in_use(&error) {
                return Err(label_taken(&record.label));
            }
            return Err(error.context("Task Scheduler registration failed"));
        }
        run(Command::new("schtasks.exe").args(["/Run", "/TN", &record.label]))
            .context("Task Scheduler start failed")?;
    }
    #[cfg(target_os = "linux")]
    return linux::register(record);
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    anyhow::bail!("Unsupported service platform: {}", record.label);
    #[cfg(any(target_os = "macos", windows))]
    Ok(())
}
pub fn unregister(record: &Record) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let target = format!("{}/{}", domain(), record.label);
        // Address our label even while launchd is transitioning it between
        // registered/running states; a preceding query is not a stop operation.
        let stopped = Command::new("launchctl")
            .args(["bootout", &target])
            .output()?;
        ensure!(
            stopped.status.success() || !installed(record)?,
            "Service manager could not unregister the gateway"
        );
        let path = agent_path(record)?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    #[cfg(windows)]
    {
        if installed(record)? {
            // /End may report that an already exited task is not running.
            let _ = Command::new("schtasks.exe")
                .args(["/End", "/TN", &record.label])
                .output();
            run(Command::new("schtasks.exe").args(["/Delete", "/TN", &record.label, "/F"]))?;
        }
    }
    #[cfg(target_os = "linux")]
    return linux::unregister(record);
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    anyhow::bail!("Unsupported service platform: {}", record.label);
    #[cfg(any(target_os = "macos", windows))]
    {
        // The registry can briefly retain an exited job after the stop command has
        // returned. Replacement must not race that asynchronous removal.
        for _ in 0..100 {
            if !installed(record)? {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        anyhow::bail!("Service removal is still pending in the user service manager")
    }
}
/// Another installation already owns this per-user service name.
pub fn label_in_use(error: &anyhow::Error) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    #[cfg(target_os = "linux")]
    {
        text.starts_with("service label ") && text.ends_with(" is already in use")
            || text.lines().any(linux::collision)
    }
    #[cfg(not(target_os = "linux"))]
    {
        text.contains("already in use")
            || text.contains("already exists")
            || text.contains("bootstrap failed: 5")
    }
}

#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
fn label_taken(label: &str) -> anyhow::Error {
    anyhow::anyhow!("Service label {label} is already in use")
}

#[cfg(target_os = "macos")]
fn write_new(path: &std::path::Path, bytes: &[u8], label: &str) -> Result<()> {
    use std::io::Write;
    let mut file = match crate::platform::private_file(path) {
        Ok(file) => file,
        Err(_) if path.exists() => return Err(label_taken(label)),
        Err(error) => return Err(error),
    };
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn installed(record: &Record) -> Result<bool> {
    #[cfg(target_os = "macos")]
    return Ok(Command::new("launchctl")
        .args(["print", &format!("{}/{}", domain(), record.label)])
        .output()?
        .status
        .success());
    #[cfg(windows)]
    return Ok(Command::new("schtasks.exe")
        .args(["/Query", "/TN", &record.label])
        .output()?
        .status
        .success());
    #[cfg(target_os = "linux")]
    return Ok(linux::state(record)?.registered);
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    anyhow::bail!("Unsupported service platform: {}", record.label)
}

/// Render an explicit operator action only; changing policy never restarts a
/// service or interrupts its calls. Quote registry values for the platform shell.
pub fn restart_command(record: &Record) -> String {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: geteuid has no pointer arguments or preconditions.
        let uid = unsafe { libc::geteuid() };
        format!(
            "launchctl kickstart -k {}",
            shell_literal(&format!("gui/{uid}/{}", record.label))
        )
    }
    #[cfg(windows)]
    {
        let label = shell_literal(&record.label);
        format!("schtasks.exe /End /TN {label}; schtasks.exe /Run /TN {label}")
    }
    #[cfg(target_os = "linux")]
    if linux::available().is_ok() {
        return linux::restart_command(record);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        format!(
            "stop the gateway, then run {} serve --config {}",
            shell_literal(&record.binary.to_string_lossy()),
            shell_literal(&record.config().to_string_lossy())
        )
    }
}

pub(super) fn log_hint(_record: &Record) -> String {
    #[cfg(target_os = "linux")]
    return linux::log_hint(_record);
    #[cfg(not(target_os = "linux"))]
    "Inspect its private service logs".into()
}

fn shell_literal(value: &str) -> String {
    #[cfg(windows)]
    let escaped = value.replace('\'', "''"); // PowerShell single-quoted string.
    #[cfg(not(windows))]
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}
#[cfg(windows)]
fn windows_xml(record: &Record) -> Result<String> {
    let identity = Command::new("whoami.exe").output()?;
    ensure!(
        identity.status.success(),
        "Cannot identify current Windows user"
    );
    let user = String::from_utf8(identity.stdout)?;
    let xml = crate::install::xml;
    let binary = &record.binary;
    Ok(format!(
        include_str!("windows-task.xml"),
        user = xml(user.trim()),
        binary = xml(&binary.to_string_lossy()),
        arguments = xml(&format!("serve --config \"{}\"", record.config().display())),
        cwd = xml(&record.release.to_string_lossy())
    ))
}
