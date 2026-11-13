use std::fs;

#[tokio::test]
async fn cleanup_preserves_docker_working_directory_and_environment() {
    let dir = tempfile::tempdir().unwrap();
    let root = mcp_gate::platform::project_path(dir.path()).unwrap();
    fs::write(root.join("relative-docker-context"), b"fixture").unwrap();
    let script = root.join(if cfg!(windows) {
        "docker-fixture.cmd"
    } else {
        "docker-fixture"
    });
    let text = if cfg!(windows) {
        "@echo off\r\nif not exist relative-docker-context exit /b 9\r\necho %MCP_DOCKER_TEST_CONTEXT%> owned-stop.txt\r\n"
    } else {
        "#!/bin/sh\n[ -f relative-docker-context ] || exit 9\nprintf '%s' \"$MCP_DOCKER_TEST_CONTEXT\" > owned-stop.txt\n"
    };
    fs::write(&script, text).unwrap();
    mcp_gate::platform::executable(&script).unwrap();
    let mut command = tokio::process::Command::new(&script);
    command
        .args(["run", "-i", "fixture"])
        .current_dir(&root)
        .env("MCP_DOCKER_TEST_CONTEXT", "fixture-context");
    if let Some(value) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", value);
    }
    let container = mcp_gate::platform::docker::Container::prepare(&mut command, &root).unwrap();
    let cidfile = command.as_std().get_args().nth(2).unwrap();
    fs::write(cidfile, "a".repeat(64)).unwrap();
    container.stop().await;
    assert_eq!(
        fs::read_to_string(root.join("owned-stop.txt"))
            .unwrap()
            .trim(),
        "fixture-context"
    );
    assert!(!std::path::Path::new(cidfile).exists());
}
