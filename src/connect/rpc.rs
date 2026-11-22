//! Track only request IDs. Methods and payloads pass through as JSON values.
use super::http::Incoming;
use serde_json::{Value, json};
use tokio::sync::mpsc::Sender;

pub(super) fn valid(message: &Value) -> bool {
    let items = items(message);
    !items.is_empty() && items.iter().all(|item| item["jsonrpc"] == "2.0")
}

fn items(message: &Value) -> &[Value] {
    message
        .as_array()
        .map_or_else(|| std::slice::from_ref(message), Vec::as_slice)
}

pub(super) fn request_id(message: &Value) -> Option<&Value> {
    message.get("method")?.as_str()?;
    message.get("id")
}

fn response_id(message: &Value) -> Option<&Value> {
    if message.get("method").is_none()
        && (message.get("result").is_some() || message.get("error").is_some())
    {
        message.get("id")
    } else {
        None
    }
}

pub(super) fn response_to<'a>(message: &'a Value, id: &Value) -> Option<&'a Value> {
    items(message)
        .iter()
        .find(|item| response_id(item) == Some(id))
}

#[derive(Default)]
pub(super) struct Replies {
    ids: Vec<Value>,
    batch: bool,
}

impl Replies {
    pub(super) fn new(message: &Value) -> Self {
        Self {
            ids: items(message)
                .iter()
                .filter_map(request_id)
                .cloned()
                .collect(),
            batch: message.is_array(),
        }
    }

    pub(super) fn observe(&mut self, message: &Value) {
        self.ids.retain(|id| response_to(message, id).is_none());
    }

    pub(super) async fn relay(mut self, incoming: Result<Incoming, String>, output: Sender<Value>) {
        if let Err(error) = self.forward(incoming, &output).await {
            self.fail(&error, &output).await;
        }
    }

    async fn forward(
        &mut self,
        incoming: Result<Incoming, String>,
        output: &Sender<Value>,
    ) -> Result<(), String> {
        let mut incoming = incoming?;
        while let Some(message) = next(&mut incoming).await? {
            self.observe(&message);
            if output.send(message).await.is_err() {
                // The bridge observes the writer's termination separately.
                return Ok(());
            }
        }
        if self.ids.is_empty() {
            Ok(())
        } else {
            Err("Gateway closed its HTTP response before replying".into())
        }
    }

    pub(super) async fn fail(self, error: &str, output: &Sender<Value>) {
        eprintln!("connect: {error}");
        if let Some(message) = self.error_response(error) {
            // A closed channel means the writer already stopped.
            drop(output.send(message).await);
        }
    }

    fn error_response(self, error: &str) -> Option<Value> {
        let mut replies = self
            .ids
            .into_iter()
            .map(|id| json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32000, "message":error}}));
        let first = replies.next()?;
        if self.batch {
            Some(Value::Array(
                std::iter::once(first).chain(replies).collect(),
            ))
        } else {
            Some(first)
        }
    }
}

pub(super) async fn next(incoming: &mut Incoming) -> Result<Option<Value>, String> {
    let message = incoming.next().await?;
    if message.as_ref().is_some_and(|message| !valid(message)) {
        Err("Gateway returned an invalid JSON-RPC message".into())
    } else {
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_keep_request_ids_and_skip_notifications_and_responses() {
        let batch = json!([
            {"jsonrpc":"2.0", "method":"tools/call", "id":"call"},
            {"jsonrpc":"2.0", "method":"tools/list", "id":3},
            {"jsonrpc":"2.0", "method":"notifications/initialized"},
            {"jsonrpc":"2.0", "id":7, "result":{}}
        ]);
        let mut replies = Replies::new(&batch);
        replies.observe(&json!({"jsonrpc":"2.0", "id":3, "result":{}}));
        replies.observe(&json!({"jsonrpc":"2.0", "method":"ping", "id":"call"}));
        let expected =
            json!({"jsonrpc":"2.0", "id":"call", "error":{"code":-32000, "message":"failed"}});
        assert_eq!(replies.error_response("failed"), Some(json!([expected])));
        assert_eq!(
            Replies::new(&batch[0]).error_response("failed"),
            Some(expected)
        );
        for item in &batch.as_array().unwrap()[2..] {
            assert_eq!(Replies::new(item).error_response("failed"), None);
        }
    }
}
