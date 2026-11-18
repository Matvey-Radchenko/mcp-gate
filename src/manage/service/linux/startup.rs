//! Interpret systemd's service state after an installer start request.
use super::{Record, label, manager, unit_name};
use std::time::Duration;

const DIAGNOSTICS: &str = "ActiveState,SubState,Result,ExecMainStatus,NRestarts";
const STARTUP: &str = "ActiveState,SubState,Result,ExecMainStatus,NRestarts,ExecMainCode,MainPID";

pub(in crate::manage::service) fn diagnostics(record: &Record) -> String {
    let unit = unit_name(record);
    if label(&unit).is_some()
        && let Ok(fields) = manager::show(&unit, DIAGNOSTICS)
    {
        return format_diagnostics(&fields);
    }
    "systemd --user service state unavailable".into()
}

pub(in crate::manage::service) async fn startup_failure(record: &Record) -> Option<String> {
    let unit = unit_name(record);
    label(&unit)?;
    // Let HTTP polling continue if the read-only user manager query stalls.
    let query = manager::show_async(&unit, STARTUP);
    match tokio::time::timeout(Duration::from_secs(2), query).await {
        Ok(Ok(fields)) => failed_start(&fields).then(|| format_diagnostics(&fields)),
        Ok(Err(error)) => {
            crate::manage::trace::log(&format!("systemctl startup query: {error}"));
            None
        }
        Err(_) => {
            crate::manage::trace::log("systemctl startup query timed out");
            None
        }
    }
}

pub(super) fn failed_start(fields: &manager::Properties) -> bool {
    let get = |key: &str| fields.get(key).map_or("", String::as_str);
    let positive = |key| get(key).parse::<u64>().is_ok_and(|value| value > 0);
    if get("ActiveState") == "failed"
        || matches!(get("SubState"), "auto-restart" | "auto-restart-queued")
        || positive("NRestarts")
    {
        return true;
    }
    // An inactive or starting unit can have no PID before its first spawn.
    // ExecMainCode is zero before an exit and nonzero even for a clean exit.
    get("MainPID") == "0"
        && (positive("ExecMainCode")
            || positive("ExecMainStatus")
            || !matches!(get("Result"), "" | "success"))
}

fn format_diagnostics(fields: &manager::Properties) -> String {
    let safe: Vec<_> = DIAGNOSTICS
        .split(',')
        .filter_map(|key| {
            let value = fields.get(key)?;
            (!value.is_empty()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
            .then(|| format!("{key}={value}"))
        })
        .collect();
    if safe.is_empty() {
        "systemd --user service state unavailable".into()
    } else {
        safe.join("; ")
    }
}
