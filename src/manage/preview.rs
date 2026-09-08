use crate::clients::{Binding, Candidate};
use serde_json::{Value, json};
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), redact(v))).collect())
        }
        Value::Array(values) => Value::Array(values.iter().map(redact).collect()),
        Value::Null | Value::Bool(_) => value.clone(),
        _ => json!("<hidden>"),
    }
}
pub fn diff(candidate: &Candidate) -> Value {
    let mut after = candidate.binding.direct.clone();
    if let Some(map) = after.as_object_mut() {
        for field in ["command", "args", "env", "environment", "env_vars", "cwd"] {
            map.remove(field);
        }
        map.insert("url".into(), json!("<gateway>"));
        map.insert(
            "authentication".into(),
            json!("<private helper or file reference>"),
        );
    }
    json!({"before":redact(&candidate.binding.before),"after":redact(&after)})
}
pub fn target(binding: &Binding) -> Value {
    json!({"client":binding.client,"name":binding.name,"source":binding.source,"target":binding.target})
}
