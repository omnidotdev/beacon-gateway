//! Telegram message processing (background task)

use std::sync::Arc;

use futures::StreamExt as _;

use super::media::extract_media_file_refs;
use super::types::TelegramMessage;
use crate::api::ApiState;
use crate::channels::{Attachment, Channel, IncomingMessage};
use crate::context::{ContextBuilder, ContextConfig};
use crate::db::MessageRole;
use crate::hooks::{HookAction, HookEvent};

/// Build an `IncomingMessage` from a Telegram webhook message
fn telegram_to_incoming(message: &TelegramMessage, content: &str) -> IncomingMessage {
    let sender_id = message
        .from
        .as_ref()
        .map_or_else(|| message.chat.id.to_string(), |u| u.id.to_string());

    let sender_name = message
        .from
        .as_ref()
        .map_or_else(|| "Unknown".to_string(), |u| u.first_name.clone());

    let is_dm = message.chat.chat_type == "private";

    IncomingMessage {
        id: message.message_id.to_string(),
        channel_id: message.chat.id.to_string(),
        sender_id,
        sender_name,
        content: content.to_string(),
        is_dm,
        reply_to: None,
        attachments: vec![],
        thread_id: message
            .message_thread_id
            .map(|id| id.to_string()),
        callback_data: None,
    }
}

/// Accumulated tool call from streaming chunks
#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Process a Telegram message (runs in background task)
///
/// Mirrors the full agent loop from `daemon.rs` — hooks, pairing, tools, events.
/// When `account_id` is `Some`, uses the per-account channel from the registry
/// and scopes session keys by account.
#[allow(clippy::too_many_lines)]
pub(crate) async fn process_telegram_message(
    state: Arc<ApiState>,
    message: TelegramMessage,
    text: String,
    has_media: bool,
    account_id: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Extract media file IDs for download
    let media_refs = extract_media_file_refs(&message);

    // Build content with attachment metadata
    let content = if has_media {
        let mut parts = vec![text.clone()];
        if message.photo.is_some() {
            parts.push("[Photo attached]".to_string());
        }
        if let Some(doc) = &message.document {
            parts.push(format!(
                "[Document: {}]",
                doc.file_name.as_deref().unwrap_or("file")
            ));
        }
        if let Some(audio) = &message.audio {
            parts.push(format!(
                "[Audio: {}]",
                audio.title.as_deref().unwrap_or("audio")
            ));
        }
        if message.video.is_some() {
            parts.push("[Video attached]".to_string());
        }
        if message.voice.is_some() {
            parts.push("[Voice message]".to_string());
        }
        parts.join("\n")
    } else {
        text
    };

    let mut msg = telegram_to_incoming(&message, &content);

    // Download media files and build attachments
    if !media_refs.is_empty() {
        if let Some(telegram) = &state.telegram {
            for media_ref in &media_refs {
                match telegram.download_file(&media_ref.file_id).await {
                    Ok((data, _file_path)) => {
                        msg.attachments.push(Attachment::from_data(
                            data,
                            media_ref.mime_type.clone(),
                            media_ref.filename.clone(),
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(
                            file_id = %media_ref.file_id,
                            error = %e,
                            "failed to download Telegram media, keeping text annotation"
                        );
                    }
                }
            }
        }
    }

    tracing::info!(
        chat_id = message.chat.id,
        from = %msg.sender_name,
        has_media,
        "Telegram webhook message received"
    );

    let Some(synapse) = &state.synapse else {
        tracing::warn!("no Synapse client configured for Telegram webhook");
        return Ok(());
    };

    // Resolve the Telegram channel — per-account from registry or default
    let telegram = if let Some(ref aid) = account_id {
        state
            .telegram_registry
            .as_ref()
            .and_then(|r| r.get(aid))
            .map(|a| &a.channel)
    } else {
        state.telegram.as_ref()
    };

    let Some(telegram) = telegram else {
        tracing::warn!("no Telegram client configured");
        return Ok(());
    };

    // Pairing check (if pairing manager is available)
    if let Some(ref pm) = state.pairing_manager {
        let allowed = pm.is_allowed(&msg.sender_id, "telegram").unwrap_or(false);
        if !allowed {
            match pm.policy() {
                crate::security::DmPolicy::Open => {}
                crate::security::DmPolicy::Disabled => {
                    tracing::debug!(sender = %msg.sender_id, "DM policy is disabled, ignoring message");
                    return Ok(());
                }
                crate::security::DmPolicy::Allowlist => {
                    tracing::debug!(sender = %msg.sender_id, "sender not on allowlist");
                    return Ok(());
                }
                crate::security::DmPolicy::Pairing => {
                    // Check for pairing code verification
                    let trimmed = msg.content.trim().to_uppercase();
                    if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
                        if let Ok(true) = pm.verify_pairing(&msg.sender_id, "telegram", &trimmed) {
                            let _ = telegram
                                .send_message(
                                    message.chat.id,
                                    "Pairing successful! You can now send messages.",
                                    None,
                                )
                                .await;
                            // Fall through to process the message
                        } else {
                            return Ok(());
                        }
                    } else if let Ok(Some(code)) =
                        pm.generate_pairing_code(&msg.sender_id, "telegram")
                    {
                        let _ = telegram
                            .send_message(
                                message.chat.id,
                                &format!(
                                    "Please enter the pairing code to start messaging.\n\nYour code: {code}\n\n(This code expires in 10 minutes)"
                                ),
                                None,
                            )
                            .await;
                        return Ok(());
                    }
                }
            }
        }
    }

    // Hook: message:received
    if let Some(ref hm) = state.hook_manager {
        let hook_event = HookEvent::new(HookAction::MessageReceived, "telegram", &msg);
        let hook_result = hm.trigger(&hook_event).await;

        if hook_result.skip_processing {
            tracing::debug!("hook skipped processing for Telegram webhook message");
            return Ok(());
        }

        if let Some(ref reply) = hook_result.reply {
            let _ = telegram
                .send_message(message.chat.id, reply, Some(message.message_id))
                .await;
            if hook_result.skip_agent {
                return Ok(());
            }
        }
    }

    // Find or create user and session
    let user = state.user_repo.find_or_create(&msg.sender_id)?;

    // Scope session key by account_id for multi-account isolation
    let session_channel_id = match &account_id {
        Some(aid) if aid != "default" => format!("{aid}:{}", msg.channel_id),
        _ => msg.channel_id.clone(),
    };
    let session = state.session_repo.find_or_create(
        &user.id,
        "telegram",
        &session_channel_id,
        &state.persona_id,
    )?;

    // Publish beacon.conversation.started for new sessions
    match state.session_repo.message_count(&session.id) {
        Ok(0) => {
            crate::events::publish(crate::events::build_conversation_started_event(
                &session.id,
                "telegram",
                &msg.sender_id,
            ));
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!("failed to check message count for session {}: {e}", session.id);
        }
    }

    // Extract thread_id for threading support
    let thread_id = msg.thread_id.as_deref();

    // Store user message with thread context
    if let Err(e) = state.session_repo.add_message_with_thread(
        &session.id,
        MessageRole::User,
        &content,
        thread_id,
    ) {
        tracing::warn!(error = %e, "failed to store user message");
    }

    // Run session compaction if needed (non-fatal on failure)
    if let Some(ref compactor) = state.session_compactor {
        if let Ok(count) = state.session_repo.message_count(&session.id) {
            if compactor.needs_compaction(count) {
                if let Err(e) = compactor
                    .compact(
                        &session.id,
                        &state.session_repo,
                        &state.memory_repo,
                        state.indexer.as_deref(),
                        &user.id,
                    )
                    .await
                {
                    tracing::warn!(error = %e, "session compaction failed, proceeding normally");
                }
            }
        }
    }

    // Build context with thread support
    let context_config = ContextConfig {
        max_messages: 20,
        max_tokens: 4000,
        persona_id: state.persona_id.clone(),
        max_memories: 10,
        persona_system_prompt: state.persona_system_prompt.clone(),
    };
    let context_builder = ContextBuilder::new(context_config);
    let mut built_context = context_builder.build_with_thread(
        &session.id,
        &user.id,
        user.life_json_path.as_deref(),
        &state.session_repo,
        &state.user_repo,
        Some((&state.memory_repo, msg.content.as_str())),
        thread_id,
    );

    if let Ok(ctx) = &built_context {
        tracing::debug!(
            session = %session.id,
            estimated_tokens = ctx.estimated_tokens,
            message_count = ctx.messages.len(),
            "built conversation context"
        );
    }

    // Publish beacon.message.received event
    crate::events::publish(
        crate::events::OmniEvent::new(
            "beacon.message.received",
            &msg.sender_id,
            serde_json::json!({
                "channel": "telegram",
                "messageId": msg.id,
                "userId": msg.sender_id,
            }),
        )
        .with_subject(&msg.sender_id),
    );

    // Load per-group config for reaction overrides
    let group_config = if message.chat.chat_type == "group" || message.chat.chat_type == "supergroup" {
        state.telegram_group_repo.get(&msg.channel_id).ok().flatten()
    } else {
        None
    };

    // Resolve per-account config for reaction overrides
    let account_config = account_id.as_ref().and_then(|aid| {
        state
            .telegram_config
            .as_ref()
            .map(|c| c.resolve_for_account(aid))
    });

    // Acknowledge message with reaction (per-group > per-account > global config)
    let reaction_level = group_config
        .as_ref()
        .and_then(|gc| gc.reaction_level.as_deref())
        .and_then(crate::config::ReactionLevel::from_str_value)
        .or_else(|| account_config.as_ref().map(|c| c.reaction_level))
        .or_else(|| state.telegram_config.as_ref().map(|c| c.reaction_level))
        .unwrap_or(crate::config::ReactionLevel::Ack);

    let global_ack = account_config
        .as_ref()
        .map(|c| c.ack_reaction.as_str())
        .or_else(|| state.telegram_config.as_ref().map(|c| c.ack_reaction.as_str()))
        .unwrap_or("\u{1F440}");
    let ack_emoji = group_config
        .as_ref()
        .and_then(|gc| gc.ack_reaction.as_deref())
        .unwrap_or(global_ack);

    if reaction_level != crate::config::ReactionLevel::Off {
        if let Err(e) = telegram.add_reaction(&msg.channel_id, &msg.id, ack_emoji).await {
            tracing::debug!(error = %e, "ack reaction failed");
        }
    }

    // Process attachments to augment message content
    let content_with_attachments = if msg.attachments.is_empty() {
        content.clone()
    } else if let Some(ref ap) = state.attachment_processor {
        let attachment_text = ap
            .process_attachments(&msg.attachments)
            .await
            .unwrap_or_default();
        if attachment_text.is_empty() {
            content.clone()
        } else {
            format!("{content}\n\n{attachment_text}")
        }
    } else {
        content.clone()
    };

    // Inject knowledge based on user message
    if let Ok(ref mut ctx) = built_context {
        if !state.persona_knowledge.is_empty() {
            let max_knowledge_tokens = state.max_context_tokens / 4;
            let selected = crate::knowledge::select_knowledge(
                &state.persona_knowledge,
                &content_with_attachments,
                max_knowledge_tokens,
            );
            if !selected.is_empty() {
                ctx.knowledge_context = crate::knowledge::format_knowledge(&selected);
            }
        }
    }

    // Build augmented prompt
    let augmented_prompt = match &built_context {
        Ok(ctx) => ctx.format_prompt(&content_with_attachments),
        Err(_) => content_with_attachments,
    };

    // Show typing indicator while processing
    let _ = telegram.send_typing(&msg.channel_id).await;

    // Hook: message:before_agent
    let mut skip_agent = false;
    if let Some(ref hm) = state.hook_manager {
        let hook_event = HookEvent::new(HookAction::BeforeAgent, "telegram", &msg)
            .with_session(&session.id);
        let hook_result = hm.trigger(&hook_event).await;

        if hook_result.skip_agent && hook_result.reply.is_some() {
            let reply = hook_result.reply.unwrap();
            let _ = telegram
                .send_message(message.chat.id, &reply, Some(message.message_id))
                .await;

            // Store and finish
            let _ = state.session_repo.add_message_with_thread(
                &session.id,
                MessageRole::Assistant,
                &reply,
                thread_id,
            );
            skip_agent = true;
        } else if let Some(hook_reply) = hook_result.reply {
            let _ = telegram
                .send_message(message.chat.id, &hook_reply, Some(message.message_id))
                .await;
        }
    }

    if skip_agent {
        return Ok(());
    }

    // Start streaming message placeholder
    let streaming_msg_id = telegram
        .send_streaming_start(
            &msg.channel_id,
            "\u{2026}",
            Some(&msg.id),
            msg.thread_id.as_deref(),
        )
        .await
        .ok();

    let response = {
        // Fetch available tools from Synapse MCP and plugins
        let tools = {
            let mut executor = crate::tools::executor::ToolExecutor::new(
                Arc::clone(synapse),
                state.plugin_manager.clone(),
            );
            if let Some(ref ct) = state.cron_tools {
                executor = executor.with_cron_tools(Arc::clone(ct));
            }
            executor.list_tools().await.ok()
        };

        // Multi-turn tool loop with streaming support
        let mut messages = vec![
            synapse_client::Message::system(&state.system_prompt_with_skills(Some(&user.id))),
            synapse_client::Message::user(&augmented_prompt),
        ];
        let mut final_response = String::new();
        let mut executor = crate::tools::executor::ToolExecutor::new(
            Arc::clone(synapse),
            state.plugin_manager.clone(),
        );
        if let Some(ref ct) = state.cron_tools {
            executor = executor.with_cron_tools(Arc::clone(ct));
        }
        let mut loop_detector = crate::tools::loop_detection::LoopDetector::default();

        for _turn in 0..10 {
            let request = synapse_client::ChatRequest {
                model: state.llm_model.clone(),
                messages: messages.clone(),
                stream: true,
                temperature: None,
                top_p: None,
                max_tokens: Some(state.llm_max_tokens),
                stop: None,
                tools: tools.clone(),
                tool_choice: None,
            };

            match synapse.chat_completion_stream(&request).await {
                Ok(mut stream) => {
                    let mut turn_text = String::new();
                    let mut pending_tool_calls: Vec<PendingToolCall> = Vec::new();
                    let mut finish_reason: Option<String> = None;

                    while let Some(event) = stream.next().await {
                        match event {
                            Ok(synapse_client::ChatEvent::ContentDelta(text)) => {
                                turn_text.push_str(&text);
                                // Stream update to Telegram (rate limiter handles throttling)
                                if let Some(ref mid) = streaming_msg_id {
                                    let _ = telegram
                                        .send_streaming_update(
                                            &msg.channel_id,
                                            mid,
                                            &turn_text,
                                        )
                                        .await;
                                }
                            }
                            Ok(synapse_client::ChatEvent::ToolCallStart { index, id, name }) => {
                                let idx = index as usize;
                                while pending_tool_calls.len() <= idx {
                                    pending_tool_calls.push(PendingToolCall::default());
                                }
                                pending_tool_calls[idx].id = id;
                                pending_tool_calls[idx].name = name;
                            }
                            Ok(synapse_client::ChatEvent::ToolCallDelta { index, arguments }) => {
                                let idx = index as usize;
                                if idx < pending_tool_calls.len() {
                                    pending_tool_calls[idx].arguments.push_str(&arguments);
                                }
                            }
                            Ok(synapse_client::ChatEvent::Done { finish_reason: fr, .. }) => {
                                finish_reason = fr;
                                break;
                            }
                            Ok(synapse_client::ChatEvent::Error(e)) => {
                                tracing::error!(error = %e, "streaming error");
                                break;
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "stream event error");
                                break;
                            }
                        }
                    }

                    if !turn_text.is_empty() {
                        final_response.push_str(&turn_text);
                    }

                    // Handle tool calls
                    if finish_reason.as_deref() == Some("tool_calls") && !pending_tool_calls.is_empty() {
                        let tool_calls: Vec<synapse_client::ToolCall> = pending_tool_calls
                            .iter()
                            .map(|tc| synapse_client::ToolCall {
                                id: tc.id.clone(),
                                tool_type: "function".to_owned(),
                                function: synapse_client::FunctionCall {
                                    name: tc.name.clone(),
                                    arguments: tc.arguments.clone(),
                                },
                            })
                            .collect();

                        let assistant_content = if turn_text.is_empty() {
                            serde_json::Value::Null
                        } else {
                            serde_json::Value::String(turn_text)
                        };

                        messages.push(synapse_client::Message {
                            role: "assistant".to_owned(),
                            content: assistant_content,
                            tool_calls: Some(tool_calls.clone()),
                            tool_call_id: None,
                        });

                        let mut should_break = false;
                        for tc in &tool_calls {
                            let result = executor
                                .execute(&tc.function.name, &tc.function.arguments)
                                .await
                                .unwrap_or_else(|e| format!("Error: {e}"));

                            let severity = loop_detector.record(
                                &tc.function.name,
                                &tc.function.arguments,
                                &result,
                            );
                            match severity {
                                crate::tools::loop_detection::LoopSeverity::CircuitBreaker => {
                                    tracing::warn!(tool = %tc.function.name, "circuit breaker: tool loop detected");
                                    messages.push(synapse_client::Message::tool(
                                        &tc.id,
                                        "Error: Circuit breaker triggered — this tool has been called too many times with the same arguments. Please try a different approach.",
                                    ));
                                    should_break = true;
                                    break;
                                }
                                crate::tools::loop_detection::LoopSeverity::Critical => {
                                    tracing::warn!(tool = %tc.function.name, "critical: possible tool loop");
                                    messages.push(synapse_client::Message::tool(&tc.id, &result));
                                    messages.push(synapse_client::Message::system(
                                        "Warning: You appear to be in a loop calling the same tool repeatedly. Please try a different approach or provide a final answer.",
                                    ));
                                }
                                crate::tools::loop_detection::LoopSeverity::Warning => {
                                    tracing::info!(tool = %tc.function.name, "warning: repeated tool call pattern");
                                    messages.push(synapse_client::Message::tool(&tc.id, &result));
                                }
                                crate::tools::loop_detection::LoopSeverity::None => {
                                    messages.push(synapse_client::Message::tool(&tc.id, &result));
                                }
                            }

                            let tool_success = !result.starts_with("Error: ");
                            crate::events::publish(
                                crate::events::build_tool_executed_event(
                                    &session.id,
                                    &tc.function.name,
                                    tool_success,
                                    &msg.sender_id,
                                ),
                            );
                        }

                        if should_break {
                            break;
                        }
                        continue;
                    }

                    break;
                }
                Err(e) => {
                    tracing::error!(error = %e, "synapse stream error");
                    final_response =
                        "Sorry, I encountered an error processing your message.".to_string();
                    break;
                }
            }
        }

        // Finalize streaming message
        if let Some(ref mid) = streaming_msg_id {
            let _ = telegram
                .send_streaming_end(&msg.channel_id, mid, &final_response)
                .await;
        }

        final_response
    };

    // Hook: message:after_agent
    let response = if let Some(ref hm) = state.hook_manager {
        let hook_event = HookEvent::new(HookAction::AfterAgent, "telegram", &msg)
            .with_session(&session.id)
            .with_response(&response);
        let hook_result = hm.trigger(&hook_event).await;
        hook_result.modified_response.unwrap_or(response)
    } else {
        response
    };

    // Store assistant response with thread context
    if let Err(e) = state.session_repo.add_message_with_thread(
        &session.id,
        MessageRole::Assistant,
        &response,
        thread_id,
    ) {
        tracing::warn!(error = %e, "failed to store assistant message");
    }

    // Send response via Telegram (only if streaming didn't already deliver it)
    if streaming_msg_id.is_none() {
        if let Err(e) = telegram
            .send_message(message.chat.id, &response, Some(message.message_id))
            .await
        {
            tracing::error!(error = %e, "failed to send Telegram response");
        }
    }

    // Mark complete with reaction (per-group > per-account > global config)
    if reaction_level != crate::config::ReactionLevel::Off {
        let global_done = account_config
            .as_ref()
            .map(|c| c.done_reaction.as_str())
            .or_else(|| state.telegram_config.as_ref().map(|c| c.done_reaction.as_str()))
            .unwrap_or("\u{2705}");
        let done_emoji = group_config
            .as_ref()
            .and_then(|gc| gc.done_reaction.as_deref())
            .unwrap_or(global_done);
        if let Err(e) = telegram
            .add_reaction(&msg.channel_id, &msg.id, done_emoji)
            .await
        {
            tracing::debug!(error = %e, "done reaction failed");
        }
    }

    // Publish beacon.message.processed event
    crate::events::publish(
        crate::events::OmniEvent::new(
            "beacon.message.processed",
            &msg.sender_id,
            serde_json::json!({
                "channel": "telegram",
                "messageId": msg.id,
                "userId": msg.sender_id,
            }),
        )
        .with_subject(&msg.sender_id),
    );

    // Publish beacon.conversation.ended event
    crate::events::publish(crate::events::build_conversation_ended_event(
        &session.id,
        "telegram",
        &msg.sender_id,
    ));

    Ok(())
}
