//! Deterministic Anthropic SSE fixture. It has no network client or account credentials.
use axum::{Json, Router, response::IntoResponse, routing::post};
use serde_json::{Value, json};
fn event(kind: &str, value: Value) -> String {
    format!("event: {kind}\ndata: {value}\n\n")
}
async fn messages(Json(request): Json<Value>) -> impl IntoResponse {
    ([("content-type", "text/event-stream")], response(&request))
}
fn response(request: &Value) -> String {
    let result = tool_result(request);
    let done = result.is_some_and(|result| result["is_error"] != true);
    let name = request["tools"].as_array().and_then(|tools| {
        tools.iter().find_map(|tool| {
            tool["name"]
                .as_str()
                .filter(|name| name.ends_with("_state"))
        })
    });
    let mut body = event(
        "message_start",
        json!({"type":"message_start","message":{
        "id":"msg_fixture","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-5",
        "stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
    );
    let tool = result.is_none() && name.is_some();
    if tool {
        body += &event(
            "content_block_start",
            json!({"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"tool_fixture","name":name,"input":{}}}),
        );
        body += &event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,
            "delta":{"type":"input_json_delta","partial_json":"{\"value\":\"offline-model-fixture\"}"}}),
        );
    } else {
        body += &event(
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        );
        body += &event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta",
                "text":if done { "fixture complete" } else if result.is_some() {
                    "fixture tool failed; no replay"
                } else { "fixture MCP tool is not available" }}}),
        );
    }
    body += &event(
        "content_block_stop",
        json!({"type":"content_block_stop","index":0}),
    );
    body += &event(
        "message_delta",
        json!({"type":"message_delta","delta":{"stop_reason":if tool {"tool_use"} else {"end_turn"},"stop_sequence":null},"usage":{"output_tokens":1}}),
    );
    body += &event("message_stop", json!({"type":"message_stop"}));
    body
}
fn tool_result(request: &Value) -> Option<&Value> {
    for message in request["messages"].as_array()? {
        if let Some(content) = message["content"].as_array()
            && let Some(result) = content
                .iter()
                .find(|c| c["type"] == "tool_result" && c["tool_use_id"] == "tool_fixture")
        {
            return Some(result);
        }
    }
    None
}

#[test]
fn fixture_never_reports_success_without_its_tool_result() {
    assert!(response(&json!({})).contains("fixture MCP tool is not available"));
    for result in [
        json!({"type":"tool_result","tool_use_id":"unrelated"}),
        json!({"type":"tool_result","tool_use_id":"tool_fixture","is_error":true}),
    ] {
        assert!(
            !response(&json!({"messages":[{"content":[result]}]})).contains("fixture complete")
        );
    }
    assert!(
        response(&json!({"messages":[{"content":[{
            "type":"tool_result","tool_use_id":"tool_fixture","content":"fixture response"
        }]}]}))
        .contains("fixture complete")
    );
    let failed = response(
        &json!({"tools":[{"name":"mcp__fixture__state"}],"messages":[{"content":[{
            "type":"tool_result","tool_use_id":"tool_fixture","is_error":true
        }]}]}),
    );
    assert!(failed.contains("fixture tool failed; no replay"));
    assert!(!failed.contains("\"type\":\"tool_use\""));
}
pub async fn start() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/messages", post(messages)),
        )
        .await
        .unwrap();
    });
    (url, handle)
}
