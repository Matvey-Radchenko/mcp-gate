//! Per-user service registration. Never installs a system service or kills a client.
use super::store::Record;
use anyhow::{Context, Result, ensure};
#[cfg(target_os = "macos")]
use std::path::PathBuf;
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
fn run(command: &mut Command) -> Result<()> {
    let output = command.output().context("Cannot run service manager")?;
    ensure!(
        output.status.success(),
        "Service manager failed ({}); inspect status before retrying",
        output.status
    );
    Ok(())
}
pub fn register(record: &Record) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let path = agent_path(record)?;
        std::fs::create_dir_all(path.parent().context("Missing LaunchAgents directory")?)?;
        ensure!(
            !path.exists(),
            "LaunchAgent already exists; refusing to overwrite"
        );
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
        super::store::atomic(&path, plist.as_bytes())?;
        run(Command::new("launchctl")
            .args(["bootstrap", &domain()])
            .arg(path))?;
    }
    #[cfg(windows)]
    {
        let task = record.release.join("task.xml");
        super::store::atomic(&task, windows_xml(record)?.as_bytes())?;
        run(Command::new("schtasks.exe")
            .args(["/Create", "/TN", &record.label, "/XML"])
            .arg(&task))?;
        run(Command::new("schtasks.exe").args(["/Run", "/TN", &record.label]))?;
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    anyhow::bail!("Unsupported service platform: {}", record.label);
    Ok(())
}
pub fn unregister(record: &Record) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let target = format!("{}/{}", domain(), record.label);
        if installed(record)? {
            run(Command::new("launchctl").args(["bootout", &target]))?;
        }
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
    #[cfg(not(any(target_os = "macos", windows)))]
    anyhow::bail!("Unsupported service platform: {}", record.label);
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
    #[cfg(not(any(target_os = "macos", windows)))]
    anyhow::bail!("Unsupported service platform: {}", record.label)
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
        r#"<?xml version="1.0" encoding="UTF-8"?><Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
<Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
<Principals><Principal id="User"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure></Settings>
<Actions Context="User"><Exec><Command>{binary}</Command><Arguments>serve --config &quot;{config}&quot;</Arguments><WorkingDirectory>{cwd}</WorkingDirectory></Exec></Actions></Task>"#,
        user = xml(user.trim()),
        binary = xml(&binary.to_string_lossy()),
        config = xml(&record.config().to_string_lossy()),
        cwd = xml(&record.release.to_string_lossy())
    ))
}
