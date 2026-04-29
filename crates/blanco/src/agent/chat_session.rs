use anyhow::Result;
use futures::StreamExt;
use gpui::{Context, EventEmitter, Task, WeakEntity, Window};
use smol::channel::Sender;
use std::collections::HashMap;
use std::sync::Arc;

use super::chat_types::{
    ChatCommand, ChatEvent, ChatMessage, LoadingState, MessageRole, ToolCallData,
};
use super::tool_handlers::ToolMode;
use crate::result_ext::ResultExt;
use database::DatabaseService;
use gpui_component::input::InputState;
use llm::{FunctionCall, LLMProvider, ToolCall, chat::ChatMessage as LlmChatMessage, chat::Tool};

/// Context for creating a ChatSession with database/editor access
pub struct ChatSessionContext {
    pub input_state: Option<WeakEntity<InputState>>,
    pub connection_id: Option<i64>,
    pub database_name: Option<String>,
}

impl ChatSessionContext {
    pub fn new() -> Self {
        Self {
            input_state: None,
            connection_id: None,
            database_name: None,
        }
    }

    pub fn with_input_state(mut self, input_state: WeakEntity<InputState>) -> Self {
        self.input_state = Some(input_state);
        self
    }

    pub fn with_connection(mut self, connection_id: i64, database_name: String) -> Self {
        self.connection_id = Some(connection_id);
        self.database_name = Some(database_name);
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
    pub llm: Option<Arc<Box<dyn LLMProvider>>>,
    pub model_name: String,
    #[allow(dead_code)]
    pub provider_name: String,
    pub loading_state: LoadingState,
    #[allow(dead_code)]
    pub streaming_message_id: Option<String>,
    pub tool_registry: Option<std::sync::Arc<super::tool_handlers::AgentToolRegistry>>,
    pub tool_mode: ToolMode,
    pub input_state: Option<WeakEntity<InputState>>,
    pub connection_id: Option<i64>,
    pub database_name: Option<String>,
    pub current_message_task: Option<Task<Result<String>>>,
    pending_approvals: HashMap<String, smol::channel::Sender<bool>>,
}

impl ChatSession {
    /// Create a new ChatSession with an LLM instance
    pub fn new(
        llm: Arc<Box<dyn LLMProvider>>,
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
                super::tool_handlers::AgentToolRegistry::new(),
            )),
            tool_mode: ToolMode::default(),
            input_state: context.input_state,
            connection_id: context.connection_id,
            database_name: context.database_name,
            current_message_task: None,
            pending_approvals: HashMap::new(),
        }
    }

    pub fn add_message(&mut self, message: ChatMessage, cx: &mut Context<Self>) {
        self.messages.push(message.clone());
        cx.emit(ChatEvent::MessageAdded { message });
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
        include_str!("system_prompt.md").to_string()
    }

    pub fn send_message(
        &mut self,
        user_message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = ChatMessage::user(user_message.clone());
        self.add_message(message.clone(), cx);

        // Check if this is a command
        if let Some(command) = ChatCommand::parse(&user_message) {
            let task = self.handle_command(command, cx);
            self.current_message_task = Some(task);
            return;
        }

        // Handle regular message based on available LLM
        if let Some(llm) = &self.llm {
            let task = self.send_llm_message(llm.clone(), &user_message, window, cx);
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
        llm: Arc<Box<dyn LLMProvider>>,
        user_message: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let system_prompt = self.get_system_prompt();
        let messages = self.messages.clone();
        let model_name = self.model_name.clone();
        let user_message = user_message.to_string();
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
                user_message,
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
                .update(async_cx, |session, cx| {
                    session.set_loading_state(final_state, cx);
                    session.current_message_task = None;
                })
                .log_err();

            response
        })
    }

    /// Process messages in a loop to handle dynamic tool call sequences with real-time UI updates
    #[allow(clippy::too_many_arguments)]
    async fn process_message_loop_with_realtime_ui(
        chat_session_handle: gpui::WeakEntity<ChatSession>,
        llm: Arc<Box<dyn LLMProvider>>,
        system_prompt: String,
        initial_messages: Vec<ChatMessage>,
        model_name: String,
        user_message: String,
        ui_sender: Sender<ChatMessage>,
        async_cx: &mut gpui::AsyncWindowContext,
    ) -> Result<String> {
        // Build initial request messages with conversation history
        let mut request_messages =
            Self::build_request_messages(&system_prompt, &initial_messages, &user_message);

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

            // Use tokio runtime for the llm crate's async methods
            let llm_clone = llm.clone();
            let messages_clone = request_messages.clone();
            let tools_clone = tools.clone();

            // Spawn the LLM call on the tokio runtime
            let response = gpui_tokio::Tokio::spawn(async_cx, async move {
                // Call the llm crate with tools
                llm_clone
                    .chat_with_tools(&messages_clone, Some(tools_clone.as_slice()))
                    .await
            })
            .await
            .map_err(|e| anyhow::anyhow!("Tokio join error: {}", e))?
            .map_err(|e| anyhow::anyhow!("LLM error: {}", e))?;

            // Get response text, tool calls, and usage
            let response_text = response.text().unwrap_or_default();
            let tool_calls = response.tool_calls();
            if let Some(usage) = response.usage() {
                accumulated_prompt_tokens = accumulated_prompt_tokens.max(usage.prompt_tokens);
                accumulated_completion_tokens += usage.completion_tokens;
            }

            // Check if there are tool calls
            if let Some(calls) = tool_calls {
                if !calls.is_empty() {
                    let tool_calls_count = calls.len();

                    tracing::debug!(
                        "Tool calls detected on iteration {}, processing {} tool calls",
                        loop_count,
                        tool_calls_count
                    );

                    // Set loading state to processing tools
                    chat_session_handle
                        .update(async_cx, |session, cx| {
                            session.set_loading_state(LoadingState::ProcessingTools, cx);
                        })
                        .log_err();

                    // Process tool calls with real-time UI updates
                    request_messages = Self::process_tool_calls_with_realtime_ui(
                        &chat_session_handle,
                        &calls,
                        request_messages,
                        &response_text,
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
                        "Completed processing {} tool calls on iteration {}, total messages: {}",
                        tool_calls_count,
                        loop_count,
                        request_messages.len()
                    );
                } else {
                    // Normal completion
                    tracing::debug!("Normal completion received on iteration {}", loop_count);
                    let mut final_message =
                        ChatMessage::assistant(response_text.clone(), model_name);
                    if accumulated_prompt_tokens > 0 || accumulated_completion_tokens > 0 {
                        final_message = final_message
                            .with_usage(accumulated_prompt_tokens, accumulated_completion_tokens);
                    }
                    if let Err(e) = ui_sender.send(final_message).await {
                        tracing::error!("Failed to send chat message to UI: {}", e);
                    }
                    return Ok(response_text);
                }
            } else {
                // Normal completion
                tracing::debug!("Normal completion received on iteration {}", loop_count);
                let mut final_message = ChatMessage::assistant(response_text.clone(), model_name);
                if accumulated_prompt_tokens > 0 || accumulated_completion_tokens > 0 {
                    final_message = final_message
                        .with_usage(accumulated_prompt_tokens, accumulated_completion_tokens);
                }
                if let Err(e) = ui_sender.send(final_message).await {
                    tracing::error!("Failed to send chat message to UI: {}", e);
                }
                return Ok(response_text);
            }
        }
    }

    /// Process tool calls with real-time UI updates
    async fn process_tool_calls_with_realtime_ui(
        chat_session_handle: &gpui::WeakEntity<ChatSession>,
        tool_calls: &[ToolCall],
        mut current_messages: Vec<LlmChatMessage>,
        assistant_content: &str,
        model_name: &str,
        ui_sender: &Sender<ChatMessage>,
        async_cx: &mut gpui::AsyncWindowContext,
    ) -> Result<Vec<LlmChatMessage>> {
        if tool_calls.is_empty() {
            return Err(anyhow::anyhow!("Tool calls array is empty"));
        }

        let mut successful_tool_calls = 0;
        let mut failed_tool_calls = 0;

        // Create tool call data for UI display
        let mut tool_call_data: Vec<ToolCallData> = tool_calls
            .iter()
            .map(|tc| ToolCallData {
                id: tc.id.clone(),
                tool_name: tc.function.name.clone(),
                arguments: tc.function.arguments.clone(),
                result: None,
                summary: None,
            })
            .collect();

        // Create and immediately emit assistant message with tool calls
        let assistant_ui_message =
            ChatMessage::assistant(assistant_content.to_string(), model_name.to_string())
                .with_tool_calls(tool_call_data.clone());
        if let Err(e) = ui_sender.send(assistant_ui_message).await {
            tracing::error!("Failed to send assistant message to UI: {}", e);
        }

        // Add the assistant's message to the conversation
        current_messages.push(
            LlmChatMessage::assistant()
                .content(assistant_content)
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
            let (tool_registry, input_state, connection_id, database_name, db_service) =
                match chat_session_handle.update_in(async_cx, |session, _window, cx| {
                    let db_service = DatabaseService::global(cx);
                    Ok::<_, anyhow::Error>((
                        session.tool_registry.clone(),
                        session.input_state.clone(),
                        session.connection_id,
                        session.database_name.clone(),
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

                        current_messages
                            .push(LlmChatMessage::user().content(&error_content).build());

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

                        current_messages
                            .push(LlmChatMessage::user().content(&error_content).build());

                        failed_tool_calls += 1;
                        continue;
                    }
                };

            let tool_context = super::tool_handlers::ToolContext {
                db_service,
                connection_id,
                database_name,
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

            // Update tool call data with result and summary
            if let Some(tc_data) = tool_call_data.get_mut(index) {
                tc_data.result = Some(result.function.arguments.clone());
                tc_data.summary = Some(summary.clone());
            }

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

            // Add tool result as a message to conversation (using user role for tool results)
            current_messages.push(
                LlmChatMessage::user()
                    .content(&result.function.arguments)
                    .build(),
            );

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

    /// Build initial request messages from conversation history
    fn build_request_messages(
        system_prompt: &str,
        messages: &[ChatMessage],
        user_message: &str,
    ) -> Vec<LlmChatMessage> {
        let mut request_messages = vec![LlmChatMessage::user().content(system_prompt).build()];

        // Add conversation history
        for message in messages {
            let provider_message = match message.role {
                MessageRole::User => LlmChatMessage::user()
                    .content(message.content.to_string())
                    .build(),
                MessageRole::Assistant => LlmChatMessage::assistant()
                    .content(message.content.to_string())
                    .build(),
                MessageRole::System => LlmChatMessage::user()
                    .content(message.content.to_string())
                    .build(),
                MessageRole::Tool => LlmChatMessage::user()
                    .content(message.content.to_string())
                    .build(),
                MessageRole::ToolRequest => LlmChatMessage::user()
                    .content(message.content.to_string())
                    .build(),
            };
            request_messages.push(provider_message);
        }

        // Add the current user message
        request_messages.push(LlmChatMessage::user().content(user_message).build());

        request_messages
    }

    pub fn is_generating(&self) -> bool {
        self.current_message_task.is_some() || self.loading_state.is_loading()
    }

    pub fn abort(&mut self, cx: &mut Context<Self>) {
        if let Some(task) = self.current_message_task.take() {
            drop(task);
        }
        self.set_loading_state(LoadingState::Idle, cx);
    }
}

impl EventEmitter<ChatEvent> for ChatSession {}
