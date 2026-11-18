use super::*;

#[test]
fn rewrites_program_and_matching_argv0_while_preserving_typed_settings() {
    let root = tempfile::tempdir().unwrap();
    let plist = root.path().join("fixture.plist");
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>local.mcp-gate.fixture</string>
<key>Program</key><string>/legacy/mcp-gate</string>
<key>ProgramArguments</key><array><string>/legacy/mcp-gate</string><string>serve</string><string>--config</string><string>/private/config.toml</string><string>--header</string><string>fixture only</string></array>
<key>EnvironmentVariables</key><dict><key>FIXTURE</key><string>private fixture</string></dict>
<key>WorkingDirectory</key><string>/legacy directory</string>
<key>ThrottleInterval</key><integer>44</integer>
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
</dict></plist>"#;
    fs::write(&plist, xml).unwrap();
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "binary1"])
        .arg(&plist)
        .output()
        .unwrap();
    assert!(output.status.success());
    let definition = Definition {
        bytes: fs::read(&plist).unwrap(),
        path: Some(plist.clone()),
    };
    let old = Path::new("/legacy/mcp-gate");
    let target = Path::new("/managed path/mcp-gate");
    assert_eq!(identity(&plist).unwrap().1, old);
    fs::write(&plist, rewrite(&definition, old, target).unwrap()).unwrap();
    assert_eq!(identity(&plist).unwrap().1, target);
    assert_eq!(
        arguments(&plist).unwrap(),
        [
            target.to_str().unwrap(),
            "serve",
            "--config",
            "/private/config.toml",
            "--header",
            "fixture only",
        ]
    );
    let converted = Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-"])
        .arg(&plist)
        .output()
        .unwrap();
    assert!(converted.status.success());
    let after = String::from_utf8(converted.stdout).unwrap();
    assert_eq!(after.matches("/managed path/mcp-gate").count(), 2);
    for expected in [
        "private fixture",
        "/legacy directory",
        "<integer>44</integer>",
        "SuccessfulExit",
        "<false/>",
    ] {
        assert!(after.contains(expected));
    }
}
