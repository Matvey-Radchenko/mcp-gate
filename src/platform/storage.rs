use anyhow::{Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

pub fn private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    #[cfg(windows)]
    {
        fs::create_dir(path)?;
        private_permissions(path)?;
    }
    Ok(())
}

pub fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    // No private bytes are written until the DACL is restricted.
    #[cfg(windows)]
    private_permissions(path)?;
    Ok(file)
}

pub fn private_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(windows)]
    acl(path, false)?;
    Ok(())
}

pub fn validate_private(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink(),
        "Private file must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            meta.permissions().mode() & 0o077 == 0,
            "Private file must have mode 600"
        );
    }
    #[cfg(windows)]
    acl(path, true)?;
    Ok(())
}

pub fn executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    ensure!(path.is_file(), "Executable is missing");
    Ok(())
}

#[cfg(windows)]
fn acl(path: &Path, check: bool) -> Result<()> {
    // File paths travel as data, never interpolated into PowerShell source.
    let script = if check {
        r#"$ErrorActionPreference='Stop'; $p=$env:MCP_GATE_ACL_PATH;
$a=Get-Acl -LiteralPath $p; $u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;
if (!$a.AreAccessRulesProtected) { exit 2 };
foreach($r in $a.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier])) {
 if ($r.AccessControlType -eq 'Allow' -and $r.IdentityReference.Value -notin @($u,'S-1-5-18')) { exit 3 }
}"#
    } else {
        r#"$ErrorActionPreference='Stop'; $p=$env:MCP_GATE_ACL_PATH;
$u=[Security.Principal.WindowsIdentity]::GetCurrent().User;
$a=Get-Acl -LiteralPath $p; $a.SetAccessRuleProtection($true,$false);
foreach($r in @($a.Access)) { [void]$a.RemoveAccessRuleSpecific($r) };
$inherit=if ((Get-Item -LiteralPath $p).PSIsContainer) {'ContainerInherit,ObjectInherit'} else {'None'};
foreach($id in @($u,[Security.Principal.SecurityIdentifier]'S-1-5-18')) {
 $r=New-Object Security.AccessControl.FileSystemAccessRule($id,'FullControl',$inherit,'None','Allow');
 $a.AddAccessRule($r)
}; Set-Acl -LiteralPath $p -AclObject $a"#
    };
    let result = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("MCP_GATE_ACL_PATH", path)
        .output()?;
    ensure!(
        result.status.success(),
        "Cannot establish/verify private Windows file permissions"
    );
    Ok(())
}
