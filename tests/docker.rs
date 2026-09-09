#![cfg(feature = "test-backend")]
#[allow(dead_code, reason = "Shared isolated MCP fixtures")]
mod support;
use serde_json::json;
use std::{
    collections::BTreeSet,
    fs,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Docker {
    binary: String,
    label: String,
    baseline: Option<String>,
}
impl Docker {
    fn run(&self, args: &[&str]) -> Output {
        let mut child = Command::new(&self.binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Fixture Docker command exceeded 30 seconds");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    fn text(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "Fixture Docker command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn ids(&self) -> BTreeSet<String> {
        self.text(&[
            "ps",
            "--all",
            "--quiet",
            "--no-trunc",
            "--filter",
            &format!("label={}", self.label),
        ])
        .lines()
        .map(str::to_owned)
        .collect()
    }
    fn baseline_alive(&self) {
        assert_eq!(
            self.text(&[
                "inspect",
                "--format",
                "{{.State.Running}}",
                self.baseline.as_ref().unwrap()
            ]),
            "true"
        );
    }
}
impl Drop for Docker {
    fn drop(&mut self) {
        // Unique per-test labels and returned IDs identify only our fixtures.
        let output = self.run(&[
            "ps",
            "--all",
            "--quiet",
            "--no-trunc",
            "--filter",
            &format!("label={}", self.label),
        ]);
        let mut ids: Vec<_> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        ids.extend(self.baseline.take());
        for id in ids {
            if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
                let _ = self.run(&["rm", "--force", &id]);
            }
        }
    }
}

#[tokio::test]
#[ignore = "Requires DOCKER_BINARY and an already-pulled Linux Node DOCKER_IMAGE; creates only isolated local fixture containers"]
async fn owned_containers_are_lazy_isolated_and_never_replayed_after_crash() {
    let mut docker = Docker {
        binary: std::env::var("DOCKER_BINARY").unwrap(),
        label: format!("mcp-gate-fixture={}", uuid::Uuid::new_v4()),
        baseline: None,
    };
    let image = std::env::var("DOCKER_IMAGE").unwrap();
    assert!(
        image.contains("@sha256:"),
        "Select an already-reviewed image digest"
    );
    docker.baseline = Some(docker.text(&[
        "run",
        "--rm",
        "--detach",
        "--pull=never",
        &image,
        "node",
        "-e",
        "setTimeout(() => {}, 180000)",
    ]));
    let mut h = support::Harness::generic("session", 4, 30, 10).await;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fixture.cjs");
    fs::write(&script, include_str!("support/docker_backend.cjs")).unwrap();
    let mut backend = mcp_gate::config::Config::load(&h.config).unwrap().backend;
    backend.command = docker.binary.clone().into();
    backend.docker = true;
    backend.version = "1.0.0".into();
    backend.args = vec![
        "run".into(),
        "--rm".into(),
        "-i".into(),
        "--pull=never".into(),
        "--label".into(),
        docker.label.clone(),
        "--mount".into(),
        format!(
            "type=bind,src={},dst=/fixture.cjs,readonly",
            script.display()
        ),
        image,
        "node".into(),
        "/fixture.cjs".into(),
    ];
    for name in [
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
        "DOCKER_CONFIG",
        "DOCKER_TLS_VERIFY",
        "DOCKER_CERT_PATH",
    ] {
        if let Ok(value) = std::env::var(name) {
            backend.env.insert(name.into(), value);
        }
    }
    h.replace_backend(backend).await;
    assert!(
        docker.ids().is_empty(),
        "Discovery container must be removed"
    );
    docker.baseline_alive();
    let a = h.session().await;
    let b = h.session().await;
    assert!(
        docker.ids().is_empty(),
        "Initialize/discovery must stay lazy"
    );
    assert_eq!(
        support::mock_value(&a.call("state", json!({})).await)["count"],
        1
    );
    let first = docker.ids();
    assert_eq!(first.len(), 1);
    assert_eq!(
        support::mock_value(&a.call("state", json!({})).await)["count"],
        2
    );
    assert_eq!(
        support::mock_value(&b.call("state", json!({})).await)["count"],
        1
    );
    assert_eq!(docker.ids().len(), 2);
    a.close().await;
    h.workers(1).await;
    let remaining = docker.ids();
    assert_eq!(remaining.len(), 1);
    assert!(first.is_disjoint(&remaining));
    assert_eq!(
        support::mock_value(&b.call("state", json!({})).await)["count"],
        2
    );
    docker.baseline_alive();
    docker.text(&["kill", remaining.first().unwrap()]);
    assert!(support::content(&b.call("state", json!({})).await).contains("NOT retried"));
    b.close().await;
    h.workers(0).await;
    assert!(
        docker.ids().is_empty(),
        "A failed session must not recreate its container"
    );
    let config = mcp_gate::config::Config::load(&h.config).unwrap();
    assert!(
        fs::read_dir(&config.state_dir)
            .unwrap()
            .all(|entry| { entry.unwrap().path().extension().is_none_or(|e| e != "cid") }),
        "Confirmed removed containers must not leave false recovery files"
    );
    docker.baseline_alive();
    h.stop();
}
