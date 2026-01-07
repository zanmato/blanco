use anyhow::Result;
use futures::StreamExt;
use gpui::{Context, EventEmitter, Task, WeakEntity, Window};
use smol::channel::Sender;
use std::sync::Arc;

use super::chat_types::{
    ChatCommand, ChatEvent, ChatMessage, LoadingState, MessageRole, SqlContext,
};
use super::tool_handlers::ToolMode;
use blanco_core::chat_provider::{ChatCompletionRequest, ChatProvider, ProviderError, ToolResult};
use database::DatabaseService;
use gpui_component::input::InputState;

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
    pub provider: Option<Arc<dyn ChatProvider<Error = ProviderError>>>,
    pub model_name: String,
    pub provider_name: String,
    pub loading_state: LoadingState,
    pub sql_context: SqlContext,
    #[allow(dead_code)]
    pub streaming_message_id: Option<String>,
    // New fields for tool execution
    pub tool_registry: Option<std::sync::Arc<super::tool_handlers::AgentToolRegistry>>,
    pub input_state: Option<WeakEntity<InputState>>,
    pub connection_id: Option<i64>,
    pub database_name: Option<String>,
    pub current_message_task: Option<Task<Result<String>>>,
}

#[allow(dead_code)]
impl ChatSession {
    /// Create a new ChatSession with a provider
    pub fn new(
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
        provider_name: String,
        model_name: String,
        context: ChatSessionContext,
    ) -> Self {
        Self {
            messages: Vec::new(),
            provider: Some(provider),
            provider_name,
            model_name,
            loading_state: LoadingState::Idle,
            sql_context: SqlContext::empty(),
            streaming_message_id: None,
            tool_registry: Some(std::sync::Arc::new(
                super::tool_handlers::AgentToolRegistry::new(),
            )),
            input_state: context.input_state,
            connection_id: context.connection_id,
            database_name: context.database_name,
            current_message_task: None,
        }
    }

    /// Create a mock ChatSession for testing
    pub fn with_mock() -> Self {
        Self {
            messages: Vec::new(),
            provider: None,
            provider_name: "Mock".to_string(),
            model_name: "mock-gpt-4".to_string(),
            loading_state: LoadingState::Idle,
            sql_context: SqlContext::empty(),
            streaming_message_id: None,
            tool_registry: Some(std::sync::Arc::new(
                super::tool_handlers::AgentToolRegistry::new(),
            )),
            input_state: None,
            connection_id: None,
            database_name: None,
            current_message_task: None,
        }
    }

    /// Create a ChatSession without a provider (for deferred initialization)
    pub fn new_empty() -> Self {
        Self {
            messages: Vec::new(),
            provider: None,
            provider_name: "Unknown".to_string(),
            model_name: "unknown".to_string(),
            loading_state: LoadingState::Idle,
            sql_context: SqlContext::empty(),
            streaming_message_id: None,
            tool_registry: None,
            input_state: None,
            connection_id: None,
            database_name: None,
            current_message_task: None,
        }
    }

    /// Set the provider after creation
    pub fn set_provider(
        &mut self,
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
        provider_name: String,
        model_name: String,
    ) {
        self.provider = Some(provider);
        self.provider_name = provider_name;
        self.model_name = model_name;
    }

    pub fn add_message(&mut self, message: ChatMessage, cx: &mut Context<Self>) {
        self.messages.push(message.clone());

        // Emit message added event
        cx.emit(ChatEvent::MessageAdded { message });
    }

    pub fn set_loading_state(&mut self, new_state: LoadingState, cx: &mut Context<Self>) {
        let old_state = self.loading_state.clone();
        self.loading_state = new_state.clone();
        cx.emit(ChatEvent::LoadingStateChanged {
            old_state,
            new_state,
        });
    }

    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.streaming_message_id = None;
    }

    pub fn update_sql_context(&mut self, context: SqlContext) {
        self.sql_context = context;
    }

    pub fn set_tool_mode(&mut self, mode: ToolMode, cx: &mut Context<Self>) {
        self.tool_registry = Some(Arc::new(
            crate::agent::tool_handlers::AgentToolRegistry::with_mode(mode),
        ));
        cx.notify();
    }

    pub fn get_system_prompt(&self) -> String {
        self.sql_context.to_system_prompt()
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

        // Handle regular message based on available provider
        if let Some(provider) = &self.provider {
            let task = self.send_provider_message(provider.clone(), &user_message, window, cx);
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

    fn send_provider_message(
        &mut self,
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
        user_message: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let system_prompt = self.sql_context.to_system_prompt();
        let messages = self.messages.clone();
        let model_name = self.model_name.clone();
        let user_message = user_message.to_string();
        let provider_clone = provider.clone();

        cx.spawn_in(window, async move |chat_session_handle, async_cx| {
            // Create an async channel for real-time UI updates
            let (tx, rx) = smol::channel::unbounded::<ChatMessage>();

            // Set initial loading state to connecting
            let _ = chat_session_handle.update(async_cx, |session, cx| {
                session.set_loading_state(LoadingState::Connecting, cx);
            });

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
                provider_clone,
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

            let _ = chat_session_handle.update(async_cx, |session, cx| {
                session.set_loading_state(final_state, cx);
                session.current_message_task = None;
            });

            response
        })
    }

    /// Process messages in a loop to handle dynamic tool call sequences with real-time UI updates
    #[allow(clippy::too_many_arguments)]
    async fn process_message_loop_with_realtime_ui(
        chat_session_handle: gpui::WeakEntity<ChatSession>,
        provider: Arc<dyn ChatProvider<Error = ProviderError>>,
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
            let _ = chat_session_handle.update(async_cx, |session, cx| {
                session.set_loading_state(LoadingState::Streaming, cx);
            });

            // Get tools from session's tool registry
            let tools = chat_session_handle
                .update_in(async_cx, |session, _, _| {
                    Ok::<_, anyhow::Error>(
                        session
                            .tool_registry
                            .as_ref()
                            .map(|registry| registry.get_tool_definitions()),
                    )
                })
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();

            // Create and send request
            let request = ChatCompletionRequest {
                model: model_name.clone(),
                messages: request_messages.clone(),
                stream: false,
                temperature: 0.7,
                max_tokens: Some(2048),
                tools: tools,
                tool_choice: None,
                top_p: None,
                frequency_penalty: None,
                presence_penalty: None,
                additional_params: None,
            };

            let response = provider.chat_completion(request).await.map_err(|e| {
                anyhow::anyhow!("Chat completion failed on iteration {}: {}", loop_count, e)
            })?;

            let choice = response.choices.first().ok_or_else(|| {
                anyhow::anyhow!("No response content received on iteration {}", loop_count)
            })?;

            tracing::debug!(
                "Response received on iteration {}, finish reason: {:?}",
                loop_count,
                choice.finish_reason
            );

            // Check if we need to process tool calls
            match choice.finish_reason {
                blanco_core::chat_provider::FinishReason::ToolCalls => {
                    let tool_calls_count = choice
                        .message
                        .tool_calls
                        .as_ref()
                        .map(|t| t.len())
                        .unwrap_or(0);

                    tracing::debug!(
                        "Tool calls detected on iteration {}, processing {} tool calls",
                        loop_count,
                        tool_calls_count
                    );

                    if tool_calls_count == 0 {
                        return Err(anyhow::anyhow!(
                            "Tool calls indicated but no tool calls found on iteration {}",
                            loop_count
                        ));
                    }

                    // Set loading state to processing tools
                    let _ = chat_session_handle.update(async_cx, |session, cx| {
                        session.set_loading_state(LoadingState::ProcessingTools, cx);
                    });

                    // Process tool calls with real-time UI updates
                    request_messages = Self::process_tool_calls_with_realtime_ui(
                        &chat_session_handle,
                        &choice.message,
                        request_messages,
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
                }
                blanco_core::chat_provider::FinishReason::Stop => {
                    // Normal completion
                    tracing::debug!("Normal completion received on iteration {}", loop_count);
                    let final_message =
                        ChatMessage::assistant(choice.message.content.clone(), model_name);
                    let _ = ui_sender.send(final_message).await;
                    return Ok(choice.message.content.clone());
                }
                blanco_core::chat_provider::FinishReason::Length => {
                    return Err(anyhow::anyhow!(
                        "Response too long on iteration {}. Consider breaking down the request.",
                        loop_count
                    ));
                }
                blanco_core::chat_provider::FinishReason::ContentFilter => {
                    return Err(anyhow::anyhow!(
                        "Content filtered by provider on iteration {}",
                        loop_count
                    ));
                }
                _ => {
                    // Handle any other finish reasons
                    tracing::debug!(
                        "Unknown finish reason {:?} on iteration {}, treating as completion",
                        choice.finish_reason,
                        loop_count
                    );
                    let final_message =
                        ChatMessage::assistant(choice.message.content.clone(), model_name);
                    let _ = ui_sender.send(final_message).await;
                    return Ok(choice.message.content.clone());
                }
            }
        }
    }

    /// Process tool calls with real-time UI updates
    async fn process_tool_calls_with_realtime_ui(
        chat_session_handle: &gpui::WeakEntity<ChatSession>,
        assistant_message: &blanco_core::chat_provider::Message,
        mut current_messages: Vec<blanco_core::chat_provider::Message>,
        model_name: &str,
        ui_sender: &Sender<ChatMessage>,
        async_cx: &mut gpui::AsyncWindowContext,
    ) -> Result<Vec<blanco_core::chat_provider::Message>> {
        let tool_calls = assistant_message
            .tool_calls
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Tool calls indicated but no tool calls found"))?;

        if tool_calls.is_empty() {
            return Err(anyhow::anyhow!("Tool calls array is empty"));
        }

        let mut successful_tool_calls = 0;
        let mut failed_tool_calls = 0;

        // Create tool call data for UI display
        let mut tool_call_data: Vec<crate::agent::chat_types::ToolCallData> = tool_calls
            .iter()
            .map(|tc| {
                crate::agent::chat_types::ToolCallData {
                    id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                    result: None,  // Will be filled after execution
                    summary: None, // Will be filled after execution
                }
            })
            .collect();

        // Create and immediately emit assistant message with tool calls
        let assistant_ui_message =
            ChatMessage::assistant(assistant_message.content.clone(), model_name.to_string())
                .with_tool_calls(tool_call_data.clone());
        let _ = ui_sender.send(assistant_ui_message).await;

        // Add the assistant's tool call message to the conversation
        current_messages.push(blanco_core::chat_provider::Message {
            role: "assistant".to_string(),
            content: assistant_message.content.clone(),
            tool_call_id: None,
            tool_calls: Some(tool_calls.clone()),
            additional_data: None,
        });

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
            let tool_call_for_error = tool_call.clone();

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
                            tool_call_for_error.id.clone(),
                            model_name.to_string(),
                        );
                        let _ = ui_sender.send(tool_ui_message).await;

                        current_messages.push(blanco_core::chat_provider::Message {
                            role: "tool".to_string(),
                            content: error_content,
                            tool_call_id: Some(tool_call_for_error.id.clone()),
                            tool_calls: None,
                            additional_data: None,
                        });

                        failed_tool_calls += 1;
                        continue;
                    }
                    Err(e) => {
                        let error_content = format!("Failed to access session: {}", e);
                        let tool_ui_message = ChatMessage::tool(
                            error_content.clone(),
                            tool_call_for_error.id.clone(),
                            model_name.to_string(),
                        );
                        let _ = ui_sender.send(tool_ui_message).await;

                        current_messages.push(blanco_core::chat_provider::Message {
                            role: "tool".to_string(),
                            content: error_content,
                            tool_call_id: Some(tool_call_for_error.id.clone()),
                            tool_calls: None,
                            additional_data: None,
                        });

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

            let result: Result<ToolResult, anyhow::Error> = if let Some(registry) = tool_registry {
                Ok(registry
                    .execute_tool(&tool_call, &tool_context, async_cx)
                    .await)
            } else {
                Ok(ToolResult::error(
                    tool_call.id.clone(),
                    "No tool registry available",
                ))
            };

            match result {
                Ok(result) => {
                    tracing::debug!(
                        "Tool call {}/{} succeeded - success: {}, content length: {}",
                        index + 1,
                        tool_calls.len(),
                        result.success,
                        result.content.len()
                    );

                    // Update tool call data with result and summary
                    if let Some(tc_data) = tool_call_data.get_mut(index) {
                        tc_data.result = Some(result.content.clone());
                        tc_data.summary = result.summary.clone();
                    }

                    // Create and immediately emit tool result message for UI
                    // Use the summary if available, otherwise use the raw content
                    let display_content = result.summary.clone().unwrap_or(result.content.clone());
                    let tool_ui_message = ChatMessage::tool(
                        display_content,
                        result.tool_call_id.clone(),
                        model_name.to_string(),
                    );
                    let _ = ui_sender.send(tool_ui_message).await;

                    // Add tool result as a message to conversation
                    current_messages.push(blanco_core::chat_provider::Message {
                        role: "tool".to_string(),
                        content: result.content,
                        tool_call_id: Some(result.tool_call_id),
                        tool_calls: None,
                        additional_data: None,
                    });

                    successful_tool_calls += 1;
                }
                Err(e) => {
                    tracing::error!("Tool call {}/{} failed: {}", index + 1, tool_calls.len(), e);

                    // Update tool call data with error
                    if let Some(tc_data) = tool_call_data.get_mut(index) {
                        tc_data.result = Some(format!("Error: {}", e));
                        tc_data.summary =
                            Some(format!("{} (Failed)", tool_call_for_error.function.name));
                    }

                    // Create and immediately emit error message for UI
                    let error_content = format!("Tool call failed: {}", e);
                    let tool_ui_message = ChatMessage::tool(
                        error_content.clone(),
                        tool_call_for_error.id.clone(),
                        model_name.to_string(),
                    );
                    let _ = ui_sender.send(tool_ui_message).await;

                    // Add error result as a message to conversation
                    current_messages.push(blanco_core::chat_provider::Message {
                        role: "tool".to_string(),
                        content: error_content,
                        tool_call_id: Some(tool_call_for_error.id.clone()),
                        tool_calls: None,
                        additional_data: None,
                    });

                    failed_tool_calls += 1;
                }
            }
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
    ) -> Vec<blanco_core::chat_provider::Message> {
        let mut request_messages = vec![blanco_core::chat_provider::Message {
            role: "system".to_string(),
            content: system_prompt.to_string(),
            tool_call_id: None,
            tool_calls: None,
            additional_data: None,
        }];

        // Add conversation history
        for message in messages {
            let provider_message = match message.role {
                MessageRole::User => blanco_core::chat_provider::Message {
                    role: "user".to_string(),
                    content: message.content.clone().into(),
                    tool_call_id: None,
                    tool_calls: None,
                    additional_data: None,
                },
                MessageRole::Assistant => blanco_core::chat_provider::Message {
                    role: "assistant".to_string(),
                    content: message.content.clone().into(),
                    tool_call_id: None,
                    tool_calls: None,
                    additional_data: None,
                },
                MessageRole::System => blanco_core::chat_provider::Message {
                    role: "system".to_string(),
                    content: message.content.clone().into(),
                    tool_call_id: None,
                    tool_calls: None,
                    additional_data: None,
                },
                MessageRole::Tool => blanco_core::chat_provider::Message {
                    role: "tool".to_string(),
                    content: message.content.clone().into(),
                    tool_call_id: message.tool_call_id.clone(),
                    tool_calls: None,
                    additional_data: None,
                },
            };
            request_messages.push(provider_message);
        }

        // Add the current user message
        request_messages.push(blanco_core::chat_provider::Message {
            role: "user".to_string(),
            content: user_message.to_string(),
            tool_call_id: None,
            tool_calls: None,
            additional_data: None,
        });

        request_messages
    }

    pub fn get_last_assistant_message(&self) -> Option<&ChatMessage> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant)
    }

    pub fn get_last_user_message(&self) -> Option<&ChatMessage> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::User)
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
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
