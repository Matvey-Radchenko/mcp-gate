//! Deterministic Anthropic SSE fixture. It has no network client or account credentials.
use axum::{Json, Router, response::IntoResponse, routing::post};
use serde_json::{Value, json};
fn event(kind: &str, value: Value) -> String {
    format!("event: {kind}\ndata: {value}\n\n")
}
async fn messages(Json(request): Json<Value>) -> impl IntoResponse {
    let done = request["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|m| {
            m["content"]
                .as_array()
                .is_some_and(|content| content.iter().any(|c| c["type"] == "tool_result"))
        })
    });
    let name = request["tools"].as_array().and_then(|tools| {
        tools.iter().find_map(|tool| {
            tool["name"]
                .as_str()
                .filter(|name| name.ends_with("__state"))
        })
    });
    let mut body = event(
        "message_start",
        json!({"type":"message_start","message":{
        "id":"msg_fixture","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-5",
        "stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
    );
    let tool = !done && name.is_some();
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
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"fixture complete"}}),
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
    ([("content-type", "text/event-stream")], body)
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
