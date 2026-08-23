use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt as _;
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use gpui_component::Root;
use llm::chat::ChatMessage as LlmChatMessage;
use llm::chat::{ChatRole, Tool};
use llm::error::LLMError;

use super::agent_chat::ChatPanel;
use super::chat_session::ChatSessionContext;
use super::chat_types::MessageRole;
use super::streaming::{ChatEventStream, ChatStreamEvent, StreamingChatProvider};

/// A provider whose responses are scripted per request. Each stream waits for one token on
/// `gate` before emitting, so a test can hold a turn open while it interacts with the panel.
struct ScriptedProvider {
    responses: Mutex<VecDeque<Vec<ChatStreamEvent>>>,
    requests: Arc<Mutex<Vec<(String, Vec<LlmChatMessage>)>>>,
    gate: smol::channel::Receiver<()>,
}

#[async_trait]
impl StreamingChatProvider for ScriptedProvider {
    async fn stream_chat(
        &self,
        system_prompt: &str,
        messages: &[LlmChatMessage],
        _tools: Option<&[Tool]>,
    ) -> Result<ChatEventStream, LLMError> {
        self.requests
            .lock()
            .expect("requests lock")
            .push((system_prompt.to_string(), messages.to_vec()));
        let events = self
            .responses
            .lock()
            .expect("responses lock")
            .pop_front()
            .unwrap_or_default();
        let gate = self.gate.clone();
        Ok(Box::pin(
            futures::stream::once(async move {
                gate.recv().await.ok();
                futures::stream::iter(events.into_iter().map(Ok))
            })
            .flatten(),
        ))
    }
}

struct ChatTestHarness {
    panel: gpui::Entity<ChatPanel>,
    requests: Arc<Mutex<Vec<(String, Vec<LlmChatMessage>)>>>,
    gate: smol::channel::Sender<()>,
}

fn setup_chat_panel(
    cx: &mut TestAppContext,
    responses: Vec<Vec<ChatStreamEvent>>,
) -> (ChatTestHarness, VisualTestContext) {
    cx.executor().allow_parking();

    let requests = Arc::new(Mutex::new(Vec::new()));
    let (gate_sender, gate_receiver) = smol::channel::unbounded::<()>();
    let provider = Arc::new(ScriptedProvider {
        responses: Mutex::new(responses.into()),
        requests: requests.clone(),
        gate: gate_receiver,
    });

    let mut panel: Option<gpui::Entity<ChatPanel>> = None;
    let window_handle = cx.update(|cx| {
        gpui_component::init(cx);
        gpui_tokio::init(cx);
        cx.open_window(Default::default(), |window, cx| {
            let chat_panel = cx.new(|cx| {
                ChatPanel::new(
                    provider,
                    "test-provider".to_string(),
                    "test-model".to_string(),
                    ChatSessionContext::new(),
                    window,
                    cx,
                )
            });
            panel = Some(chat_panel.clone());
            cx.new(|cx| Root::new(chat_panel, window, cx))
        })
        .expect("failed to open window")
    });

    let visual_cx = VisualTestContext::from_window(window_handle.into(), cx);
    (
        ChatTestHarness {
            panel: panel.expect("panel should be set"),
            requests,
            gate: gate_sender,
        },
        visual_cx,
    )
}

fn send_user_message(harness: &ChatTestHarness, text: &str, cx: &mut VisualTestContext) {
    let session = harness
        .panel
        .read_with(cx, |panel, _cx| panel.session.clone());
    session.update_in(cx, |session, window, cx| {
        session.send_message(text.to_string(), window, cx);
    });
}

async fn wait_for_request_count(
    harness: &ChatTestHarness,
    count: usize,
    cx: &mut VisualTestContext,
) {
    for _ in 0..100 {
        cx.run_until_parked();
        if harness.requests.lock().expect("requests lock").len() >= count {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    panic!("Provider never received request #{count}");
}

async fn wait_until_idle(harness: &ChatTestHarness, cx: &mut VisualTestContext) {
    for _ in 0..100 {
        cx.run_until_parked();
        let generating = harness
            .panel
            .read_with(cx, |panel, cx| panel.session.read(cx).is_generating());
        if !generating {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    panic!("Session never became idle");
}

#[gpui::test]
async fn test_stream_deltas_append_into_message_views(cx: &mut TestAppContext) {
    let (harness, mut cx) = setup_chat_panel(
        cx,
        vec![vec![
            ChatStreamEvent::Reasoning("thinking ".to_string()),
            ChatStreamEvent::Reasoning("hard".to_string()),
            ChatStreamEvent::Content("Hello".to_string()),
            ChatStreamEvent::Content(" world".to_string()),
        ]],
    );

    send_user_message(&harness, "hi", &mut cx);
    harness.gate.try_send(()).expect("gate send");
    wait_until_idle(&harness, &mut cx).await;

    let (content, reasoning, expanded) = harness.panel.read_with(&cx, |panel, cx| {
        let assistant = panel
            .messages
            .iter()
            .find(|entity| entity.read(cx).role == MessageRole::Assistant)
            .expect("assistant message should exist");
        let state = assistant.read(cx);
        (
            state.streamed_content().to_string(),
            state.streamed_reasoning().to_string(),
            state.thinking_expanded,
        )
    });

    assert_eq!(content, "Hello world");
    assert_eq!(reasoning, "thinking hard");
    assert!(
        !expanded,
        "thinking should auto-collapse once answer content arrives"
    );
}

#[gpui::test]
async fn test_steering_queues_message_and_replays_it_next_turn(cx: &mut TestAppContext) {
    let (harness, mut cx) = setup_chat_panel(
        cx,
        vec![
            vec![ChatStreamEvent::Content("First answer".to_string())],
            vec![ChatStreamEvent::Content("Second answer".to_string())],
        ],
    );

    send_user_message(&harness, "first question", &mut cx);
    wait_for_request_count(&harness, 1, &mut cx).await;

    // The turn is held open by the gate; sending now must queue, not cancel.
    send_user_message(&harness, "second question", &mut cx);
    cx.run_until_parked();

    let user_messages = harness.panel.read_with(&cx, |panel, cx| {
        panel
            .messages
            .iter()
            .filter(|entity| entity.read(cx).role == MessageRole::User)
            .count()
    });
    assert_eq!(user_messages, 2, "queued message should render immediately");
    assert_eq!(harness.requests.lock().expect("requests lock").len(), 1);

    harness.gate.try_send(()).expect("gate send");
    wait_for_request_count(&harness, 2, &mut cx).await;
    harness.gate.try_send(()).expect("gate send");
    wait_until_idle(&harness, &mut cx).await;

    let requests = harness.requests.lock().expect("requests lock");
    let second_request = &requests[1].1;
    let last_user = second_request
        .iter()
        .rev()
        .find(|message| message.role == ChatRole::User)
        .expect("second request should carry a user message");
    assert_eq!(last_user.content, "second question");
    let first_answer_present = second_request.iter().any(|message| {
        message.role == ChatRole::Assistant && message.content.contains("First answer")
    });
    assert!(
        first_answer_present,
        "the first turn's answer should be in the second request's history"
    );
}

#[gpui::test]
async fn test_manual_thinking_toggle_sticks(cx: &mut TestAppContext) {
    let (harness, mut cx) = setup_chat_panel(
        cx,
        vec![vec![
            ChatStreamEvent::Reasoning("pondering".to_string()),
            ChatStreamEvent::Content("Answer".to_string()),
        ]],
    );

    send_user_message(&harness, "hi", &mut cx);
    harness.gate.try_send(()).expect("gate send");
    wait_until_idle(&harness, &mut cx).await;

    let assistant = harness.panel.read_with(&cx, |panel, cx| {
        panel
            .messages
            .iter()
            .find(|entity| entity.read(cx).role == MessageRole::Assistant)
            .cloned()
            .expect("assistant message should exist")
    });

    assistant.update(&mut cx, |state, cx| {
        assert!(!state.thinking_expanded);
        state.toggle_thinking(cx);
        assert!(state.thinking_expanded);
        assert!(state.user_toggled_thinking());

        // Further stream activity must not override the user's choice.
        state.append_stream_delta("more", "", cx);
        assert!(state.thinking_expanded);
    });
}
