use anyhow::Result;
use futures::StreamExt;
use gpui::{Context, EventEmitter, Task, WeakEntity, Window};
use smol::channel::Sender;
use std::collections::HashMap;
use std::sync::Arc;

use super::chat_types::{
    ChatCommand, ChatEvent, ChatMessage, LoadingState, MessageRole, ToolCallData,
};
use super::streaming::{ChatStreamEvent, StreamingChatProvider};
use super::tool_handlers::ToolMode;
use crate::result_ext::ResultExt;
use database::{DatabaseService, DatabaseType};
use gpui_component::input::EditorState;
use llm::{FunctionCall, ToolCall, chat::ChatMessage as LlmChatMessage, chat::Tool};

/// What the tab the chat is attached to actually contains. The database it
/// talks to is the same either way, so this is separate from [`DatabaseType`]:
/// together they decide which system prompt the session gets and how the
/// tab-editing tools describe the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TabLanguage {
    /// A query tab: SQL, or Redis commands, per the connection's type.
    #[default]
    Query,
    /// A script tab: JavaScript driving the connection through `db`.
    Script,
}

/// The system prompt for a tab holding `tab_language` against `db_type`.
fn system_prompt_for(tab_language: TabLanguage, db_type: Option<DatabaseType>) -> String {
    match tab_language {
        TabLanguage::Query => {
            // Exhaustive match (no wildcard) so adding a DatabaseType variant
            // fails to compile until its agent prompt is chosen deliberately:
            // SQL dialects share the SQL prompt, key/value stores get a
            // command-oriented one.
            let prompt = match db_type {
                None
                | Some(DatabaseType::SQLite)
                | Some(DatabaseType::PostgreSQL)
                | Some(DatabaseType::MySQL)
                | Some(DatabaseType::ClickHouse)
                | Some(DatabaseType::MsSql) => include_str!("system_prompt.md"),
                Some(DatabaseType::Redis) => include_str!("redis_system_prompt.md"),
            };
            prompt.to_string()
        }
        TabLanguage::Script => script_system_prompt(db_type),
    }
}

/// The script-tab prompt is assembled rather than a fixed file: the `db` API
/// reference comes from the same tables the editor completes from, and what the
/// statement strings passed to `db` actually are depends on the connection.
fn script_system_prompt(db_type: Option<DatabaseType>) -> String {
    let supports_sql = db_type.map(|db| db.supports_sql()).unwrap_or(true);
    let dialect = match db_type {
        Some(db_type) => db_type.dialect().command_syntax_description(),
        None => "The connection's type is unknown; write portable SQL.".to_string(),
    };

    format!(
        "{}\n{}\n\nThe injected API:\n\n{}",
        include_str!("script_system_prompt.md"),
        dialect,
        crate::script_completion::api_reference(supports_sql)
    )
}

/// Context for creating a ChatSession with database/editor access
pub struct ChatSessionContext {
    pub input_state: Option<WeakEntity<EditorState>>,
    pub connection_id: Option<i64>,
    pub database_name: Option<String>,
    pub db_type: Option<DatabaseType>,
    pub tab_language: TabLanguage,
}

impl ChatSessionContext {
    pub fn new() -> Self {
        Self {
            input_state: None,
            connection_id: None,
            database_name: None,
            db_type: None,
            tab_language: TabLanguage::default(),
        }
    }

    pub fn with_input_state(mut self, input_state: WeakEntity<EditorState>) -> Self {
        self.input_state = Some(input_state);
        self
    }

    pub fn with_connection(
        mut self,
        connection_id: i64,
        database_name: String,
        db_type: DatabaseType,
    ) -> Self {
        self.connection_id = Some(connection_id);
        self.database_name = Some(database_name);
        self.db_type = Some(db_type);
        self
    }

    pub fn with_tab_language(mut self, tab_language: TabLanguage) -> Self {
        self.tab_language = tab_language;
        self
    }
}

impl Default for ChatSessionContext {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ChatSession {
    pub messages: Vec<ChatMessage>,
    pub llm: Option<Arc<dyn StreamingChatProvider>>,
    pub model_name: String,
    #[allow(dead_code)]
    pub provider_name: String,
    pub loading_state: LoadingState,
    #[allow(dead_code)]
    pub streaming_message_id: Option<String>,
    pub tool_registry: Option<std::sync::Arc<super::tool_handlers::AgentToolRegistry>>,
    pub tool_mode: ToolMode,
    pub input_state: Option<WeakEntity<EditorState>>,
    pub connection_id: Option<i64>,
    pub database_name: Option<String>,
    pub db_type: Option<DatabaseType>,
    pub tab_language: TabLanguage,
    pub current_message_task: Option<Task<Result<String>>>,
    pending_approvals: HashMap<String, smol::channel::Sender<bool>>,
    /// User messages sent while a turn is running (steering). They are already in
    /// `messages` for display; the running loop drains this queue before its next request,
    /// and any leftovers start a fresh turn when the current one finishes.
    queued_user_messages: Vec<String>,
}

impl ChatSession {
    /// Create a new ChatSession with an LLM instance
    pub fn new(
        llm: Arc<dyn StreamingChatProvider>,
        provider_name: String,
        model_name: String,
        context: ChatSessionContext,
    ) -> Self {
        Self {
            messages: Vec::new(),
            llm: Some(llm),
            provider_name,
            model_name,
            loading_state: LoadingState::Idle,
            streaming_message_id: None,
            tool_registry: Some(std::sync::Arc::new(
                super::tool_handlers::AgentToolRegistry::new(context.tab_language, context.db_type),
            )),
            tool_mode: ToolMode::default(),
            input_state: context.input_state,
            connection_id: context.connection_id,
            database_name: context.database_name,
            db_type: context.db_type,
            tab_language: context.tab_language,
            current_message_task: None,
            pending_approvals: HashMap::new(),
            queued_user_messages: Vec::new(),
        }
    }

    pub fn add_message(&mut self, message: ChatMessage, cx: &mut Context<Self>) {
        self.messages.push(message.clone());
        cx.emit(ChatEvent::MessageAdded { message });
    }

    /// Create an empty assistant message that subsequent stream deltas append into, and
    /// return its id. The panel renders it immediately so output appears as it streams.
    fn begin_streaming_message(&mut self, cx: &mut Context<Self>) -> String {
        let message = ChatMessage::assistant(String::new(), self.model_name.clone());
        let id = message.id.clone();
        self.streaming_message_id = Some(id.clone());
        self.add_message(message, cx);
        id
    }

    /// Write the fully streamed text into the message's history entry. Stream deltas only
    /// update the panel's views, so this runs once per turn (before tool execution or on
    /// completion) to make the history authoritative for future request rebuilds.
    fn finalize_streamed_text(&mut self, message_id: &str, content: &str, reasoning: &str) {
        if let Some(message) = self.messages.iter_mut().find(|m| m.id == message_id) {
            message.content = content.to_string().into();
            message.reasoning = (!reasoning.is_empty()).then(|| reasoning.to_string().into());
        }
    }

    /// Attach final token usage to the streaming message once the turn completes.
    fn complete_streaming_message(
        &mut self,
        message_id: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
        cx: &mut Context<Self>,
    ) {
        if prompt_tokens > 0 || completion_tokens > 0 {
            if let Some(message) = self.messages.iter_mut().find(|m| m.id == message_id) {
                message.metadata.prompt_tokens = Some(prompt_tokens);
                message.metadata.completion_tokens = Some(completion_tokens);
                message.metadata.tokens_used = Some(prompt_tokens + completion_tokens);
            }
        }
        self.streaming_message_id = None;
        cx.emit(ChatEvent::StreamCompleted {
            message_id: message_id.to_string(),
            prompt_tokens,
            completion_tokens,
        });
    }

    pub fn set_loading_state(&mut self, new_state: LoadingState, cx: &mut Context<Self>) {
        let old_state = self.loading_state.clone();
        self.loading_state = new_state.clone();
        cx.emit(ChatEvent::LoadingStateChanged {
            _old_state: old_state,
            new_state,
        });
    }

    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.streaming_message_id = None;
        self.queued_user_messages.clear();
    }

    pub fn set_tool_mode(&mut self, mode: ToolMode, cx: &mut Context<Self>) {
        self.tool_mode = mode;
        cx.notify();
    }

    pub fn register_pending_approval(
        &mut self,
        tool_call_id: String,
        sender: smol::channel::Sender<bool>,
    ) {
        self.pending_approvals.insert(tool_call_id, sender);
    }

    pub fn approve_tool(&mut self, tool_call_id: &str) {
        if let Some(sender) = self.pending_approvals.remove(tool_call_id) {
            let _ = sender.try_send(true);
        }
    }

    pub fn deny_tool(&mut self, tool_call_id: &str) {
        if let Some(sender) = self.pending_approvals.remove(tool_call_id) {
            let _ = sender.try_send(false);
        }
    }

    pub fn get_system_prompt(&self) -> String {
        system_prompt_for(self.tab_language, self.db_type)
    }

    pub fn send_message(
        &mut self,
        user_message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = ChatMessage::user(user_message.clone());
        self.add_message(message, cx);

        // Steering: while a turn is running the message is queued instead of starting a new
        // request. The running loop injects it before its next model call. Slash commands
        // are not parsed here since they would race the in-flight loop.
        if self.is_generating() {
            self.queued_user_messages.push(user_message);
            return;
        }

        // Check if this is a command
        if let Some(command) = ChatCommand::parse(&user_message) {
            let task = self.handle_command(command, cx);
            self.current_message_task = Some(task);
            return;
        }

        // Handle regular message based on available LLM
        if let Some(llm) = &self.llm {
            let task = self.send_llm_message(llm.clone(), window, cx);
            self.current_message_task = Some(task);
        }
    }

    fn handle_command(
        &mut self,
        command: ChatCommand,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let response = match command {
            ChatCommand::Help => ChatCommand::help_text().to_string(),
            ChatCommand::New => {
                self.clear_messages();
                cx.emit(ChatEvent::SessionCleared);
                "Started a new chat session. Message history cleared.".to_string()
            }
        };

        let message = ChatMessage::assistant(response.clone(), self.model_name.clone());
        self.add_message(message.clone(), cx);
        cx.emit(ChatEvent::MessageAdded { message });

        Task::ready(Ok(response))
    }

    fn send_llm_message(
        &mut self,
        llm: Arc<dyn StreamingChatProvider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let system_prompt = self.get_system_prompt();
        let messages = self.messages.clone();
        let model_name = self.model_name.clone();
        let llm_clone = llm.clone();

        cx.spawn_in(window, async move |chat_session_handle, async_cx| {
            // Create an async channel for real-time UI updates
            let (tx, rx) = smol::channel::unbounded::<ChatMessage>();

            // Set initial loading state to connecting
            chat_session_handle
                .update(async_cx, |session, cx| {
                    session.set_loading_state(LoadingState::Connecting, cx);
                })
                .log_err();

            // Spawn a task to listen for UI updates
            let handle_clone = chat_session_handle.clone();
            async_cx
                .spawn(async move |cx| {
                    futures::pin_mut!(rx);
                    while let Some(message) = rx.next().await {
                        if handle_clone
                            .update(cx, |chat_session, cx| {
                                chat_session.add_message(message.clone(), cx);
                            })
                            .is_ok()
                        {
                            // State updated successfully
                        }
                    }
                })
                .detach();

            // Process the message loop with real-time UI updates
            let response = Self::process_message_loop_with_realtime_ui(
                chat_session_handle.clone(),
                llm_clone,
                system_prompt,
                messages,
                model_name.clone(),
                tx,
                async_cx,
            )
            .await;

            // Reset loading state when done
            let final_state = match &response {
                Ok(_) => LoadingState::Idle,
                Err(e) => LoadingState::Error(e.to_string()),
            };

            chat_session_handle
                .update_in(async_cx, |session, window, cx| {
                    session.set_loading_state(final_state, cx);
                    session.current_message_task = None;

                    // A steering message can arrive after the loop's last queue drain but
                    // before this cleanup; it is already in history, so a fresh turn picks
                    // it up from there.
                    if !session.queued_user_messages.is_empty()
                        && let Some(llm) = session.llm.clone()
                    {
                        session.queued_user_messages.clear();
                        let task = session.send_llm_message(llm, window, cx);
                        session.current_message_task = Some(task);
                    }
                })
                .log_err();

            response
        })
    }

    /// Process messages in a loop to handle dynamic tool call sequences with real-time UI updates
    #[allow(clippy::too_many_arguments)]
    async fn process_message_loop_with_realtime_ui(
        chat_session_handle: gpui::WeakEntity<ChatSession>,
        llm: Arc<dyn StreamingChatProvider>,
        system_prompt: String,
        initial_messages: Vec<ChatMessage>,
        model_name: String,
        ui_sender: Sender<ChatMessage>,
        async_cx: &mut gpui::AsyncWindowContext,
    ) -> Result<String> {
        // Build initial request messages with conversation history (the latest user message
        // is already in the history)
        let mut request_messages = Self::build_request_messages(&initial_messages);

        let mut loop_count = 0;
        let mut accumulated_prompt_tokens: u32 = 0;
        let mut accumulated_completion_tokens: u32 = 0;
        const MAX_LOOP_ITERATIONS: usize = 10; // Prevent infinite loops

        loop {
            // Safety check to prevent infinite loops
            if loop_count >= MAX_LOOP_ITERATIONS {
                return Err(anyhow::anyhow!(
                    "Maximum tool call iterations ({}) exceeded. This may indicate a loop in tool calls.",
                    MAX_LOOP_ITERATIONS
                ));
            }
            loop_count += 1;

            tracing::debug!("Starting message loop iteration {}", loop_count);

            // Inject messages the user sent while this turn was running (steering). They are
            // already in session history/UI; here they join the LLM-side conversation.
            let queued_messages = chat_session_handle
                .update(async_cx, |session, _cx| {
                    std::mem::take(&mut session.queued_user_messages)
                })
                .ok()
                .unwrap_or_default();
            for queued_message in queued_messages {
                request_messages.push(LlmChatMessage::user().content(queued_message).build());
            }

            // Set loading state to streaming when making request
            chat_session_handle
                .update(async_cx, |session, cx| {
                    session.set_loading_state(LoadingState::Streaming, cx);
                })
                .log_err();

            // Get tools from session's tool registry
            let tools: Vec<Tool> = chat_session_handle
                .update_in(async_cx, |session, _, _| {
                    session
                        .tool_registry
                        .as_ref()
                        .map(|registry| registry.get_llm_tools())
                })
                .ok()
                .flatten()
                .unwrap_or_default();

            // Create the assistant message that streamed deltas append into; it renders
            // immediately so reasoning and answer text appear as they arrive.
            let streaming_id = chat_session_handle
                .update(async_cx, |session, cx| session.begin_streaming_message(cx))
                .map_err(|e| anyhow::anyhow!("Failed to begin streaming message: {}", e))?;

            // Open the stream on the Tokio runtime (reqwest must be polled there); the events
            // are then drained here on the GPUI foreground to update the UI incrementally.
            let mut stream = {
                let llm = llm.clone();
                let system_prompt = system_prompt.clone();
                let messages = request_messages.clone();
                let tools = tools.clone();
                gpui_tokio::Tokio::spawn(async_cx, async move {
                    llm.stream_chat(&system_prompt, &messages, Some(tools.as_slice()))
                        .await
                })
                .await
                .map_err(|e| anyhow::anyhow!("Tokio join error: {}", e))?
                .map_err(|e| anyhow::anyhow!("LLM error: {}", e))?
            };

            let mut content = String::new();
            let mut reasoning = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut iteration_prompt_tokens: u32 = 0;
            let mut iteration_completion_tokens: u32 = 0;

            while let Some(event) = stream.next().await {
                match event {
                    Ok(ChatStreamEvent::Content(delta)) => {
                        content.push_str(&delta);
                        Self::push_stream_delta(
                            &chat_session_handle,
                            &streaming_id,
                            delta,
                            String::new(),
                            async_cx,
                        );
                    }
                    Ok(ChatStreamEvent::Reasoning(delta)) => {
                        reasoning.push_str(&delta);
                        Self::push_stream_delta(
                            &chat_session_handle,
                            &streaming_id,
                            String::new(),
                            delta,
                            async_cx,
                        );
                    }
                    Ok(ChatStreamEvent::ToolCalls(calls)) => tool_calls.extend(calls),
                    Ok(ChatStreamEvent::Usage {
                        prompt_tokens,
                        completion_tokens,
                    }) => {
                        iteration_prompt_tokens = prompt_tokens;
                        iteration_completion_tokens = completion_tokens;
                    }
                    Err(e) => return Err(anyhow::anyhow!("LLM error: {}", e)),
                }
            }

            accumulated_prompt_tokens = accumulated_prompt_tokens.max(iteration_prompt_tokens);
            accumulated_completion_tokens += iteration_completion_tokens;

            {
                let streaming_id = streaming_id.clone();
                let content = content.clone();
                let reasoning = reasoning.clone();
                chat_session_handle
                    .update(async_cx, |session, _cx| {
                        session.finalize_streamed_text(&streaming_id, &content, &reasoning);
                    })
                    .log_err();
            }

            if tool_calls.is_empty() {
                // Normal completion: attach the full turn's token usage and finish.
                tracing::debug!("Normal completion received on iteration {}", loop_count);
                let id = streaming_id.clone();
                chat_session_handle
                    .update(async_cx, |session, cx| {
                        session.complete_streaming_message(
                            &id,
                            accumulated_prompt_tokens,
                            accumulated_completion_tokens,
                            cx,
                        );
                    })
                    .log_err();
                return Ok(content);
            }

            tracing::debug!(
                "Tool calls detected on iteration {}, processing {} tool calls",
                loop_count,
                tool_calls.len()
            );

            // The streamed assistant message stays as-is (tokens are only surfaced on the
            // final answer); clear the streaming marker so the next iteration starts fresh.
            chat_session_handle
                .update(async_cx, |session, _cx| {
                    session.streaming_message_id = None;
                })
                .log_err();

            chat_session_handle
                .update(async_cx, |session, cx| {
                    session.set_loading_state(LoadingState::ProcessingTools, cx);
                })
                .log_err();

            request_messages = Self::execute_tool_calls(
                &chat_session_handle,
                &tool_calls,
                request_messages,
                &content,
                &streaming_id,
                &model_name,
                &ui_sender,
                async_cx,
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to process tool calls on iteration {}: {}",
                    loop_count,
                    e
                )
            })?;

            tracing::debug!(
                "Completed processing tool calls on iteration {}, total messages: {}",
                loop_count,
                request_messages.len()
            );
        }
    }

    /// Emit newly streamed text to the panel from the async loop.
    fn push_stream_delta(
        chat_session_handle: &gpui::WeakEntity<ChatSession>,
        streaming_id: &str,
        content_delta: String,
        reasoning_delta: String,
        async_cx: &mut gpui::AsyncWindowContext,
    ) {
        let message_id = streaming_id.to_string();
        chat_session_handle
            .update(async_cx, |_session, cx| {
                cx.emit(ChatEvent::StreamDelta {
                    message_id,
                    content_delta,
                    reasoning_delta,
                });
            })
            .log_err();
    }

    /// Execute the streamed tool calls and append their results to the conversation.
    ///
    /// The assistant message that requested the tools was already streamed into the UI, so
    /// this only adds it to the LLM-side history (`current_messages`) and renders the tool
    /// request/result messages.
    #[allow(clippy::too_many_arguments)]
    async fn execute_tool_calls(
        chat_session_handle: &gpui::WeakEntity<ChatSession>,
        tool_calls: &[ToolCall],
        mut current_messages: Vec<LlmChatMessage>,
        assistant_content: &str,
        assistant_message_id: &str,
        model_name: &str,
        ui_sender: &Sender<ChatMessage>,
        async_cx: &mut gpui::AsyncWindowContext,
    ) -> Result<Vec<LlmChatMessage>> {
        if tool_calls.is_empty() {
            return Err(anyhow::anyhow!("Tool calls array is empty"));
        }

        let mut successful_tool_calls = 0;
        let mut failed_tool_calls = 0;

        // Add the assistant's (already-streamed) message to the LLM-side conversation,
        // including the tool calls it made so the model sees evidence it performed them.
        current_messages.push(
            LlmChatMessage::assistant()
                .content(assistant_content)
                .tool_use(tool_calls.to_vec())
                .build(),
        );

        // Execute each tool call individually with real-time updates
        for (index, tool_call) in tool_calls.iter().enumerate() {
            tracing::debug!(
                "Executing tool call {}/{}: {} with args: {}",
                index + 1,
                tool_calls.len(),
                tool_call.function.name,
                tool_call.function.arguments
            );

            let tool_call = tool_call.clone();

            // Get tool context and registry from session
            let (tool_registry, input_state, connection_id, database_name, db_type, db_service) =
                match chat_session_handle.update_in(async_cx, |session, _window, cx| {
                    let db_service = DatabaseService::global(cx);
                    Ok::<_, anyhow::Error>((
                        session.tool_registry.clone(),
                        session.input_state.clone(),
                        session.connection_id,
                        session.database_name.clone(),
                        session.db_type,
                        Arc::new(db_service.clone()) as Arc<dyn blanco_core::DatabaseService>,
                    ))
                }) {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => {
                        let error_content = format!("Failed to get session state: {}", e);
                        let tool_ui_message = ChatMessage::tool(
                            error_content.clone(),
                            tool_call.id.clone(),
                            model_name.to_string(),
                        );
                        if let Err(e) = ui_sender.send(tool_ui_message).await {
                            tracing::error!("Failed to send tool message to UI: {}", e);
                        }

                        current_messages.push(
                            LlmChatMessage::user()
                                .tool_result(vec![error_tool_result(&tool_call, &error_content)])
                                .build(),
                        );

                        failed_tool_calls += 1;
                        continue;
                    }
                    Err(e) => {
                        let error_content = format!("Failed to access session: {}", e);
                        let tool_ui_message = ChatMessage::tool(
                            error_content.clone(),
                            tool_call.id.clone(),
                            model_name.to_string(),
                        );
                        if let Err(e) = ui_sender.send(tool_ui_message).await {
                            tracing::error!("Failed to send tool message to UI: {}", e);
                        }

                        current_messages.push(
                            LlmChatMessage::user()
                                .tool_result(vec![error_tool_result(&tool_call, &error_content)])
                                .build(),
                        );

                        failed_tool_calls += 1;
                        continue;
                    }
                };

            let tool_context = super::tool_handlers::ToolContext {
                db_service,
                connection_id,
                database_name,
                db_type,
                input_state,
            };

            // Determine tool mode and request approval if needed
            let tool_mode = chat_session_handle
                .update(async_cx, |session, _cx| session.tool_mode)
                .ok()
                .unwrap_or_default();

            let mut denied = false;

            let always_allow = tool_registry
                .as_ref()
                .map(|r| r.always_allow(&tool_call.function.name))
                .unwrap_or(false);

            if tool_mode == ToolMode::Ask && !always_allow {
                let (approval_tx, approval_rx) = smol::channel::bounded::<bool>(1);

                let preview = tool_registry
                    .as_ref()
                    .map(|r| r.call_preview(&tool_call))
                    .unwrap_or_else(|| tool_call.function.name.clone());

                let code_block = if tool_call.function.name == "execute-sql" {
                    serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments)
                        .ok()
                        .and_then(|args| {
                            args.get("sql")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                } else {
                    None
                };

                let tool_request_message = ChatMessage::tool_request(
                    tool_call.id.clone(),
                    tool_call.function.name.clone(),
                    preview,
                    code_block,
                    model_name.to_string(),
                );

                if let Err(e) = ui_sender.send(tool_request_message).await {
                    tracing::error!("Failed to send tool request to UI: {}", e);
                }

                chat_session_handle
                    .update(async_cx, |session, _cx| {
                        session.register_pending_approval(tool_call.id.clone(), approval_tx);
                    })
                    .log_err();

                chat_session_handle
                    .update(async_cx, |session, cx| {
                        session.set_loading_state(LoadingState::AwaitingApproval, cx);
                    })
                    .log_err();

                let approved = approval_rx.recv().await.unwrap_or(false);

                chat_session_handle
                    .update(async_cx, |session, cx| {
                        session.set_loading_state(LoadingState::ProcessingTools, cx);
                    })
                    .log_err();

                if !approved {
                    denied = true;
                }
            }

            let (result, summary): (ToolCall, String) = if denied {
                let denied_result = ToolCall {
                    id: tool_call.id.clone(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: tool_call.function.name.clone(),
                        arguments: serde_json::json!({"error": "User denied tool execution"})
                            .to_string(),
                    },
                };
                (denied_result, "Denied by user".to_string())
            } else if let Some(registry) = tool_registry {
                registry
                    .execute_tool_with_summary(&tool_call, &tool_context, async_cx)
                    .await
            } else {
                let error_result = ToolCall {
                    id: tool_call.id.clone(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: tool_call.function.name.clone(),
                        arguments: serde_json::json!({"error": "No tool registry available"})
                            .to_string(),
                    },
                };
                (
                    error_result,
                    "Error: No tool registry available".to_string(),
                )
            };

            if tool_mode == ToolMode::Ask {
                chat_session_handle
                    .update(async_cx, |_session, cx| {
                        cx.emit(ChatEvent::ToolResultReady {
                            tool_call_id: tool_call.id.clone(),
                            result_summary: summary.clone(),
                        });
                    })
                    .log_err();
            } else {
                let mut tool_content = summary.clone();
                if tool_call.function.name == "execute-sql"
                    && let Some(sql) =
                        serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments)
                            .ok()
                            .and_then(|args| {
                                args.get("sql")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            })
                {
                    tool_content = format!("{}\n```sql\n{}\n```", tool_content, sql);
                }
                let tool_ui_message =
                    ChatMessage::tool(tool_content, result.id.clone(), model_name.to_string());
                if let Err(e) = ui_sender.send(tool_ui_message).await {
                    tracing::error!("Failed to send tool result to UI: {}", e);
                }
            }

            // Feed the result back with proper tool linkage (result payload in
            // `function.arguments`, tied to the request by the call id).
            current_messages.push(
                LlmChatMessage::user()
                    .tool_result(vec![result.clone()])
                    .build(),
            );

            // Record the call and its result on the assistant message in session history, so
            // the next turn's request rebuild replays the same ToolUse/ToolResult pair.
            {
                let assistant_message_id = assistant_message_id.to_string();
                let call_data = ToolCallData {
                    id: result.id.clone(),
                    tool_name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                    result: Some(result.function.arguments.clone()),
                    summary: Some(summary.clone()),
                };
                chat_session_handle
                    .update(async_cx, |session, _cx| {
                        if let Some(message) = session
                            .messages
                            .iter_mut()
                            .find(|m| m.id == assistant_message_id)
                        {
                            message
                                .tool_calls
                                .get_or_insert_with(Vec::new)
                                .push(call_data);
                        }
                    })
                    .log_err();
            }

            successful_tool_calls += 1;
        }

        tracing::debug!(
            "Processed {} tool calls: {} successful, {} failed, total messages: {}",
            tool_calls.len(),
            successful_tool_calls,
            failed_tool_calls,
            current_messages.len()
        );

        // If all tool calls failed, return an error
        if failed_tool_calls == tool_calls.len() {
            return Err(anyhow::anyhow!(
                "All {} tool calls failed",
                tool_calls.len()
            ));
        }

        // If some tool calls failed, log a warning but continue
        if failed_tool_calls > 0 {
            tracing::warn!(
                "{} out of {} tool calls failed",
                failed_tool_calls,
                tool_calls.len()
            );
        }

        Ok(current_messages)
    }

    /// Build the LLM-side conversation from chat history. The latest user message is already
    /// part of `messages`, so nothing is appended here.
    ///
    /// Tool and ToolRequest rows are display-only: the authoritative tool history lives on
    /// each assistant message's `tool_calls`, replayed as a ToolUse message followed
    /// immediately by its ToolResult so providers see the calls they made and their results
    /// with proper linkage (OpenAI requires that adjacency).
    fn build_request_messages(messages: &[ChatMessage]) -> Vec<LlmChatMessage> {
        let mut request_messages = Vec::with_capacity(messages.len());

        for message in messages {
            match message.role {
                MessageRole::User => request_messages.push(
                    LlmChatMessage::user()
                        .content(message.content.to_string())
                        .build(),
                ),
                MessageRole::Assistant => {
                    let tool_calls = message.tool_calls.as_deref().unwrap_or_default();
                    if tool_calls.is_empty() {
                        if !message.content.is_empty() {
                            request_messages.push(
                                LlmChatMessage::assistant()
                                    .content(message.content.to_string())
                                    .build(),
                            );
                        }
                        continue;
                    }

                    let requests: Vec<ToolCall> = tool_calls
                        .iter()
                        .map(|call| ToolCall {
                            id: call.id.clone(),
                            call_type: "function".to_string(),
                            function: FunctionCall {
                                name: call.tool_name.clone(),
                                arguments: call.arguments.clone(),
                            },
                        })
                        .collect();
                    let results: Vec<ToolCall> = tool_calls
                        .iter()
                        .map(|call| ToolCall {
                            id: call.id.clone(),
                            call_type: "function".to_string(),
                            function: FunctionCall {
                                name: call.tool_name.clone(),
                                arguments: call.result.clone().unwrap_or_else(|| {
                                    serde_json::json!({"error": "Tool result unavailable"})
                                        .to_string()
                                }),
                            },
                        })
                        .collect();

                    request_messages.push(
                        LlmChatMessage::assistant()
                            .content(message.content.to_string())
                            .tool_use(requests)
                            .build(),
                    );
                    request_messages.push(LlmChatMessage::user().tool_result(results).build());
                }
                MessageRole::System | MessageRole::Tool | MessageRole::ToolRequest => {}
            }
        }

        request_messages
    }

    pub fn is_generating(&self) -> bool {
        self.current_message_task.is_some() || self.loading_state.is_loading()
    }

    pub fn abort(&mut self, cx: &mut Context<Self>) {
        if let Some(task) = self.current_message_task.take() {
            drop(task);
        }
        self.streaming_message_id = None;
        self.set_loading_state(LoadingState::Idle, cx);
    }
}

/// A ToolResult entry for a call that failed before its handler could run. Every call in an
/// assistant ToolUse message must get a result on the wire or providers reject the request.
fn error_tool_result(tool_call: &ToolCall, error_message: &str) -> ToolCall {
    ToolCall {
        id: tool_call.id.clone(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: tool_call.function.name.clone(),
            arguments: serde_json::json!({ "error": error_message }).to_string(),
        },
    }
}

impl EventEmitter<ChatEvent> for ChatSession {}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::chat::{ChatRole, MessageType};

    #[test]
    fn request_messages_contain_each_history_message_once() {
        let history = vec![
            ChatMessage::user("count the users".to_string()),
            ChatMessage::assistant("There are 42 users.".to_string(), "test-model".to_string()),
            ChatMessage::user("thanks".to_string()),
        ];

        let request = ChatSession::build_request_messages(&history);

        assert_eq!(request.len(), 3);
        assert_eq!(request[0].role, ChatRole::User);
        assert_eq!(request[0].content, "count the users");
        assert_eq!(request[1].role, ChatRole::Assistant);
        assert_eq!(request[2].content, "thanks");
    }

    #[test]
    fn assistant_tool_calls_replay_as_adjacent_tool_use_and_result() {
        let mut assistant =
            ChatMessage::assistant("Checking.".to_string(), "test-model".to_string());
        assistant.tool_calls = Some(vec![ToolCallData {
            id: "call_1".to_string(),
            tool_name: "execute-sql".to_string(),
            arguments: r#"{"sql":"SELECT 1"}"#.to_string(),
            result: Some(r#"{"rows":[["1"]]}"#.to_string()),
            summary: Some("Execute SQL".to_string()),
        }]);
        let history = vec![ChatMessage::user("run it".to_string()), assistant];

        let request = ChatSession::build_request_messages(&history);

        assert_eq!(request.len(), 3);
        let MessageType::ToolUse(calls) = &request[1].message_type else {
            panic!("expected ToolUse, got {:?}", request[1].message_type);
        };
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "execute-sql");
        assert_eq!(request[1].content, "Checking.");
        let MessageType::ToolResult(results) = &request[2].message_type else {
            panic!("expected ToolResult, got {:?}", request[2].message_type);
        };
        assert_eq!(results[0].id, "call_1");
        assert_eq!(results[0].function.arguments, r#"{"rows":[["1"]]}"#);
    }

    #[test]
    fn display_only_rows_are_skipped() {
        let history = vec![
            ChatMessage::user("hello".to_string()),
            ChatMessage::tool(
                "tool output".to_string(),
                "call_1".to_string(),
                "test-model".to_string(),
            ),
            ChatMessage::assistant(String::new(), "test-model".to_string()),
        ];

        let request = ChatSession::build_request_messages(&history);

        assert_eq!(request.len(), 1);
        assert_eq!(request[0].content, "hello");
    }

    #[test]
    fn script_tabs_get_a_javascript_prompt() {
        let query = system_prompt_for(TabLanguage::Query, Some(DatabaseType::PostgreSQL));
        let script = system_prompt_for(TabLanguage::Script, Some(DatabaseType::PostgreSQL));

        assert_ne!(query, script);
        assert!(script.contains("JavaScript"), "{script}");
        assert!(script.contains("db.display"), "{script}");
        assert!(script.contains("PostgreSQL SQL"), "{script}");
        assert!(!query.contains("db.display"), "{query}");
    }

    #[test]
    fn a_script_on_a_non_sql_connection_is_told_it_writes_commands() {
        let script = system_prompt_for(TabLanguage::Script, Some(DatabaseType::Redis));

        assert!(script.contains("redis-cli"), "{script}");
        assert!(!script.contains("Redis SQL"), "{script}");
        assert!(script.contains("no bind parameters"), "{script}");
    }
}
