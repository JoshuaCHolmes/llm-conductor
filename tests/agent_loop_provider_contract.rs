//! End-to-end test of the `Provider` trait contract that the agent loop relies on.
//!
//! The real agent loop in `Repl::handle_message` calls `provider.call_with_tools()`,
//! receives a `ToolCallResponse`, executes any tool calls, then loops with the
//! results appended to history. This test scripts a fake provider through that
//! exact sequence — proving the trait surface is sufficient to drive a multi-round
//! tool-using agent without HTTP, and giving future REPL refactors a regression
//! gate against accidental contract changes.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Mutex;

use llm_conductor::providers::{Provider, ToolCallResponse, ToolDefinition};
use llm_conductor::types::{
    CapabilityTier, Message, ModelId, ModelInfo, ProviderId, Role, ToolCall,
};

#[derive(Clone)]
enum Scripted {
    Text(&'static str),
    ToolCalls(Vec<(&'static str, &'static str, &'static str)>), // (id, name, args_json)
}

struct ScriptedProvider {
    script: Mutex<std::collections::VecDeque<Scripted>>,
    seen_history_lengths: Mutex<Vec<usize>>,
}

impl ScriptedProvider {
    fn new(script: Vec<Scripted>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            seen_history_lengths: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn available_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![fake_model()])
    }

    async fn chat(&self, _model: &ModelInfo, _messages: &[Message]) -> Result<String> {
        unreachable!("agent loop uses call_with_tools for tool-capable models")
    }

    async fn chat_stream(
        &self,
        _model: &ModelInfo,
        _messages: &[Message],
        _callback: Box<dyn Fn(String) + Send>,
    ) -> Result<(String, Option<u64>)> {
        unreachable!()
    }

    async fn call_with_tools(
        &self,
        _model: &ModelInfo,
        messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<ToolCallResponse> {
        self.seen_history_lengths.lock().unwrap().push(messages.len());

        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("script exhausted"))?;

        Ok(match next {
            Scripted::Text(t) => ToolCallResponse {
                text: Some(t.to_string()),
                tool_calls: None,
                tokens: Some(10),
            },
            Scripted::ToolCalls(calls) => {
                let tool_calls: Vec<ToolCall> = calls
                    .into_iter()
                    .map(|(id, name, args)| ToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        arguments: args.to_string(),
                    })
                    .collect();
                ToolCallResponse {
                    text: None,
                    tool_calls: Some(tool_calls),
                    tokens: Some(10),
                }
            }
        })
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(true)
    }
}

fn fake_model() -> ModelInfo {
    ModelInfo {
        id: ModelId::Custom("scripted".into()),
        name: "scripted".into(),
        provider: ProviderId::Custom("test".into()),
        capability_tier: CapabilityTier::Basic,
        context_window: 8192,
        supports_vision: false,
        supports_streaming: false,
        cost_per_token: 0.0,
        supports_tool_calling: true,
    }
}

/// Drive a minimal agent loop: alternate provider call → execute tool → append
/// `Role::Tool` result → call provider again, until the provider returns text.
/// Mirrors `Repl::handle_message` without rendering / shell layers.
async fn drive_loop(
    provider: &dyn Provider,
    initial_user: &str,
    max_rounds: usize,
) -> Result<(String, Vec<Message>)> {
    let model = fake_model();
    let tools: Vec<ToolDefinition> = vec![];

    let mut history: Vec<Message> = vec![Message::user(initial_user)];

    for _round in 0..max_rounds {
        let resp = provider.call_with_tools(&model, &history, &tools).await?;

        if let Some(calls) = resp.tool_calls {
            let assistant_text = resp.text.clone().unwrap_or_default();
            history.push(Message::assistant_tool_calls(assistant_text, calls.clone()));

            for call in calls {
                let result = format!("ok:{}", call.name);
                history.push(Message::tool_result(call.id, result));
            }
            continue;
        }

        let text = resp.text.unwrap_or_default();
        history.push(Message::assistant(&text));
        return Ok((text, history));
    }

    anyhow::bail!("max_rounds={} exceeded", max_rounds)
}

#[tokio::test]
async fn single_text_response_terminates_loop() {
    let provider = ScriptedProvider::new(vec![Scripted::Text("hello world")]);
    let (text, history) = drive_loop(&provider, "hi", 5).await.unwrap();
    assert_eq!(text, "hello world");
    assert_eq!(history.len(), 2);
    assert_eq!(provider.seen_history_lengths.lock().unwrap().as_slice(), &[1]);
}

#[tokio::test]
async fn tool_call_round_trips_then_finishes() {
    let provider = ScriptedProvider::new(vec![
        Scripted::ToolCalls(vec![("c1", "bash", r#"{"cmd":"ls"}"#)]),
        Scripted::Text("done"),
    ]);
    let (text, history) = drive_loop(&provider, "list files", 5).await.unwrap();
    assert_eq!(text, "done");

    assert_eq!(history.len(), 4);
    assert!(matches!(history[0].role, Role::User));
    assert!(matches!(history[1].role, Role::Assistant));
    assert!(history[1].tool_calls.is_some());
    assert!(matches!(history[2].role, Role::Tool));
    assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
    assert!(matches!(history[3].role, Role::Assistant));

    let lens = provider.seen_history_lengths.lock().unwrap().clone();
    assert_eq!(lens, vec![1, 3]);
}

#[tokio::test]
async fn parallel_tool_calls_in_single_turn() {
    let provider = ScriptedProvider::new(vec![
        Scripted::ToolCalls(vec![
            ("a", "bash", r#"{"cmd":"echo 1"}"#),
            ("b", "bash", r#"{"cmd":"echo 2"}"#),
            ("c", "bash", r#"{"cmd":"echo 3"}"#),
        ]),
        Scripted::Text("all three ran"),
    ]);
    let (text, history) = drive_loop(&provider, "echo 3 things", 5).await.unwrap();
    assert_eq!(text, "all three ran");
    assert_eq!(history.len(), 6);

    let ids: Vec<_> = history
        .iter()
        .filter(|m| matches!(m.role, Role::Tool))
        .map(|m| m.tool_call_id.clone().unwrap())
        .collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[tokio::test]
async fn multi_round_tool_chain() {
    let provider = ScriptedProvider::new(vec![
        Scripted::ToolCalls(vec![("t1", "bash", "{}")]),
        Scripted::ToolCalls(vec![("t2", "bash", "{}")]),
        Scripted::ToolCalls(vec![("t3", "bash", "{}")]),
        Scripted::Text("converged"),
    ]);
    let (text, history) = drive_loop(&provider, "go", 10).await.unwrap();
    assert_eq!(text, "converged");
    assert_eq!(history.len(), 8);

    let lens = provider.seen_history_lengths.lock().unwrap().clone();
    assert_eq!(lens, vec![1, 3, 5, 7]);
}

#[tokio::test]
async fn round_limit_is_enforced() {
    let provider = ScriptedProvider::new(vec![
        Scripted::ToolCalls(vec![("x", "bash", "{}")]),
        Scripted::ToolCalls(vec![("x", "bash", "{}")]),
        Scripted::ToolCalls(vec![("x", "bash", "{}")]),
    ]);
    let err = drive_loop(&provider, "loop", 2).await.unwrap_err();
    assert!(err.to_string().contains("max_rounds=2"));
}

#[tokio::test]
async fn tool_result_messages_have_required_id_field() {
    // Regression gate: the agent loop *must* produce Role::Tool messages with a
    // populated tool_call_id; openai-format providers refuse to serialize without one.
    let provider = ScriptedProvider::new(vec![
        Scripted::ToolCalls(vec![("must-be-set", "bash", "{}")]),
        Scripted::Text("ok"),
    ]);
    let (_text, history) = drive_loop(&provider, "x", 5).await.unwrap();
    let tool_msg = history.iter().find(|m| matches!(m.role, Role::Tool)).unwrap();
    assert_eq!(tool_msg.tool_call_id.as_deref(), Some("must-be-set"));
    assert!(!tool_msg.tool_call_id.as_ref().unwrap().is_empty());
}
