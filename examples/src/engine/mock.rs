//! A bounded local Chat Completions fixture, never a real model.

use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub(super) struct Mock {
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Mock {
    pub async fn start() -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}/v1", listener.local_addr()?);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/chat/completions", post(answer))
            .with_state(requests.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            base_url,
            requests,
            task,
        })
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn answer(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    Json(request): Json<Value>,
) -> ([(&'static str, &'static str); 1], String) {
    let turn = {
        let mut requests = requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let turn = requests.len();
        requests.push(request);
        turn
    };
    let deltas = if turn == 0 {
        vec![
            json!({"content":"Checking the record. "}),
            json!({"tool_calls":[{"index":0,"id":"lookup-1","type":"function","function":{"name":"lookup_record","arguments":"{\"reference\":\"42\"}"}}]}),
        ]
    } else {
        vec![
            json!({"content":"Record 42 "}),
            json!({"content":"is ready."}),
        ]
    };
    let mut body = String::new();
    for delta in deltas {
        body.push_str(&format!(
            "data: {}\n\n",
            json!({"choices":[{"index":0,"delta":delta}]})
        ));
    }
    body.push_str(&format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":{},"finish_reason":if turn == 0 {"tool_calls"} else {"stop"}}]})));
    // Usage arrives in a final chunk, as with `stream_options.include_usage`.
    let usage = json!({"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":8}});
    body.push_str(&format!("data: {usage}\n\n"));
    body.push_str("data: [DONE]\n\n");
    ([("content-type", "text/event-stream")], body)
}
