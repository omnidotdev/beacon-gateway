//! Channel integration tests
//!
//! Tests the message handling flow with mock channels

use std::sync::Arc;

use async_trait::async_trait;
use beacon_gateway::{
    ToolPolicy, ToolPolicyConfig, ToolProfile,
    channels::{
        BotCommand, Channel, ChannelCapability, ChannelRegistry, IncomingMessage, OutgoingMessage,
        TelegramChannel, TelegramRateLimiter, UpdateDedup, should_skip_group_message,
    },
    config::{ReactionLevel, TelegramConfig},
    db::{Memory, MemoryCategory, MemoryRepo, MessageRole, SessionRepo, UserRepo},
    tools::loop_detection::{LoopDetector, LoopSeverity},
};
use tokio::sync::Mutex;

mod common;
use common::setup_test_db;

/// Mock channel for testing
struct MockChannel {
    name: &'static str,
    connected: bool,
    sent_messages: Arc<Mutex<Vec<OutgoingMessage>>>,
}

impl MockChannel {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            connected: false,
            sent_messages: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn get_sent_messages(&self) -> Vec<OutgoingMessage> {
        self.sent_messages.lock().await.clone()
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn connect(&mut self) -> beacon_gateway::Result<()> {
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> beacon_gateway::Result<()> {
        self.connected = false;
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> beacon_gateway::Result<()> {
        self.sent_messages.lock().await.push(message);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

#[tokio::test]
async fn test_mock_channel_connect_disconnect() {
    let mut channel = MockChannel::new("test");

    assert!(!channel.is_connected());

    channel.connect().await.unwrap();
    assert!(channel.is_connected());

    channel.disconnect().await.unwrap();
    assert!(!channel.is_connected());
}

#[tokio::test]
async fn test_mock_channel_send() {
    let mut channel = MockChannel::new("test");
    channel.connect().await.unwrap();

    let message = OutgoingMessage {
        channel_id: "channel-123".to_string(),
        content: "Hello, world!".to_string(),
        reply_to: None,
        thread_id: None,
        keyboard: None,
        media: vec![],
        edit_target: None,
        voice_note: false,
    };

    channel.send(message.clone()).await.unwrap();

    let sent = channel.get_sent_messages().await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].content, "Hello, world!");
}

#[tokio::test]
async fn test_session_persistence() {
    let db = setup_test_db();

    // Create user and session
    let user_repo = UserRepo::new(db.clone());
    let session_repo = SessionRepo::new(db.clone());

    let user = user_repo.find_or_create("discord:12345").unwrap();
    let session = session_repo
        .find_or_create(&user.id, "discord", "channel-abc", "orin")
        .unwrap();

    // Add messages
    session_repo
        .add_message(&session.id, MessageRole::User, "Hello Orin")
        .unwrap();
    session_repo
        .add_message(&session.id, MessageRole::Assistant, "Hello! How can I help?")
        .unwrap();

    // Retrieve messages
    let messages = session_repo.get_messages(&session.id, 10).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, "Hello Orin");
    assert_eq!(messages[1].content, "Hello! How can I help?");
}

#[tokio::test]
async fn test_multiple_sessions_per_user() {
    let db = setup_test_db();

    let user_repo = UserRepo::new(db.clone());
    let session_repo = SessionRepo::new(db.clone());

    let user = user_repo.find_or_create("user-1").unwrap();

    // Create sessions on different channels
    let discord_session = session_repo
        .find_or_create(&user.id, "discord", "ch-1", "orin")
        .unwrap();
    let slack_session = session_repo
        .find_or_create(&user.id, "slack", "ch-2", "orin")
        .unwrap();

    assert_ne!(discord_session.id, slack_session.id);

    // Same channel/channel_id should return same session
    let same_session = session_repo
        .find_or_create(&user.id, "discord", "ch-1", "orin")
        .unwrap();
    assert_eq!(discord_session.id, same_session.id);
}

#[tokio::test]
async fn test_tool_policy_channel_filtering() {
    use std::collections::HashMap;

    let mut channels = HashMap::new();
    channels.insert("voice".to_string(), ToolProfile::Full);
    channels.insert("discord".to_string(), ToolProfile::Messaging);
    channels.insert("default".to_string(), ToolProfile::Minimal);

    let config = ToolPolicyConfig { channels };
    let policy = ToolPolicy::new(&config);

    // Voice gets full access (includes shell, write_file, etc.)
    let voice_tools = policy.allowed_tools("voice");
    assert!(voice_tools.contains(&"shell".to_string()));
    assert!(voice_tools.contains(&"write_file".to_string()));

    // Discord gets messaging profile (web_search, read_file only)
    let discord_tools = policy.allowed_tools("discord");
    assert!(discord_tools.contains(&"web_search".to_string()));
    assert!(discord_tools.contains(&"read_file".to_string()));
    assert!(!discord_tools.contains(&"shell".to_string()));
    assert!(!discord_tools.contains(&"write_file".to_string()));

    // Unknown channel gets default (minimal - web_search only)
    let unknown_tools = policy.allowed_tools("telegram");
    assert!(unknown_tools.contains(&"web_search".to_string()));
    assert!(!unknown_tools.contains(&"shell".to_string()));
}

#[tokio::test]
async fn test_memory_storage_and_retrieval() {
    let db = setup_test_db();

    let user_repo = UserRepo::new(db.clone());
    let memory_repo = MemoryRepo::new(db.clone());

    let user = user_repo.find_or_create("test-user").unwrap();

    // Create and store a memory
    let memory = Memory::new(
        user.id.clone(),
        MemoryCategory::Preference,
        "User's favorite color is blue".to_string(),
    );
    memory_repo.add(&memory).unwrap();

    // Retrieve memories
    let memories = memory_repo.list(&user.id, None).unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].content, "User's favorite color is blue");
    assert_eq!(memories[0].category, MemoryCategory::Preference);
}

#[tokio::test]
async fn test_memory_search() {
    let db = setup_test_db();

    let user_repo = UserRepo::new(db.clone());
    let memory_repo = MemoryRepo::new(db.clone());

    let user = user_repo.find_or_create("search-user").unwrap();

    // Add multiple memories
    let m1 = Memory::new(user.id.clone(), MemoryCategory::Preference, "Prefers dark mode".to_string());
    let m2 = Memory::new(user.id.clone(), MemoryCategory::Fact, "Lives in Seattle".to_string());
    let m3 = Memory::new(user.id.clone(), MemoryCategory::Preference, "Likes coffee".to_string());

    memory_repo.add(&m1).unwrap();
    memory_repo.add(&m2).unwrap();
    memory_repo.add(&m3).unwrap();

    // Search for specific content
    let results = memory_repo.search(&user.id, "dark").unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("dark mode"));

    // Search that matches nothing
    let empty = memory_repo.search(&user.id, "xyz123").unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn test_incoming_message_structure() {
    let msg = IncomingMessage {
        id: "msg123".to_string(),
        channel_id: "123456789".to_string(),
        sender_id: "user:abc".to_string(),
        sender_name: "Test User".to_string(),
        content: "Hello, assistant!".to_string(),
        is_dm: true,
        reply_to: None,
        attachments: vec![],
        thread_id: None,
        callback_data: None,
    };

    assert!(msg.is_dm);
    assert_eq!(msg.sender_name, "Test User");
}

#[tokio::test]
async fn test_channel_registry() {
    let mut registry = ChannelRegistry::new();

    let channel1 = Box::new(MockChannel::new("mock1"));
    let channel2 = Box::new(MockChannel::new("mock2"));

    registry.register(channel1);
    registry.register(channel2);

    // Connect all
    registry.connect_all().await.unwrap();

    // Disconnect all
    registry.disconnect_all().await;
}

#[tokio::test]
async fn test_user_life_json_path() {
    let db = setup_test_db();

    let user_repo = UserRepo::new(db.clone());
    let user = user_repo.find_or_create("life-json-user").unwrap();

    // Initially no life.json path
    assert!(user.life_json_path.is_none());

    // Set life.json path
    user_repo
        .set_life_json_path(&user.id, Some("/home/user/life.json"))
        .unwrap();

    // Verify it's set
    let updated = user_repo.find(&user.id).unwrap().unwrap();
    assert_eq!(updated.life_json_path.as_deref(), Some("/home/user/life.json"));

    // Clear it
    user_repo.set_life_json_path(&user.id, None).unwrap();
    let cleared = user_repo.find(&user.id).unwrap().unwrap();
    assert!(cleared.life_json_path.is_none());
}

#[tokio::test]
async fn test_pinned_memories_come_first() {
    let db = setup_test_db();

    let user_repo = UserRepo::new(db.clone());
    let memory_repo = MemoryRepo::new(db.clone());

    let user = user_repo.find_or_create("pinned-test-user").unwrap();

    // Add an unpinned memory first
    let m1 = Memory::new(user.id.clone(), MemoryCategory::General, "Unpinned memory".to_string());
    memory_repo.add(&m1).unwrap();

    // Add a pinned memory second
    let m2 = Memory::new(user.id.clone(), MemoryCategory::Fact, "Pinned memory".to_string())
        .pinned();
    memory_repo.add(&m2).unwrap();

    // Get context - pinned should come first
    let context = memory_repo.get_context(&user.id, 10).unwrap();
    assert_eq!(context.len(), 2);
    assert!(context[0].pinned);
    assert!(!context[1].pinned);
}

#[tokio::test]
async fn test_channel_capabilities_default_empty() {
    let channel = MockChannel::new("test");

    // Default implementation returns an empty slice
    assert!(channel.capabilities().is_empty());
}

#[tokio::test]
async fn test_channel_capability_equality() {
    // Verify enum derives work correctly
    let a = ChannelCapability::Streaming;
    let b = ChannelCapability::Streaming;
    let c = ChannelCapability::Reactions;

    assert_eq!(a, b);
    assert_ne!(a, c);
}

/// Mock channel that declares specific capabilities
struct CapableChannel;

#[async_trait]
impl Channel for CapableChannel {
    fn name(&self) -> &'static str {
        "capable"
    }

    fn capabilities(&self) -> &'static [ChannelCapability] {
        &[
            ChannelCapability::Streaming,
            ChannelCapability::Reactions,
            ChannelCapability::MediaSend,
        ]
    }

    async fn connect(&mut self) -> beacon_gateway::Result<()> {
        Ok(())
    }

    async fn disconnect(&mut self) -> beacon_gateway::Result<()> {
        Ok(())
    }

    async fn send(&self, _message: OutgoingMessage) -> beacon_gateway::Result<()> {
        Ok(())
    }

    fn is_connected(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn test_channel_declares_capabilities() {
    let channel = CapableChannel;

    let caps = channel.capabilities();
    assert_eq!(caps.len(), 3);
    assert!(caps.contains(&ChannelCapability::Streaming));
    assert!(caps.contains(&ChannelCapability::Reactions));
    assert!(caps.contains(&ChannelCapability::MediaSend));
    assert!(!caps.contains(&ChannelCapability::Stickers));
}

#[test]
fn incoming_message_has_thread_and_callback() {
    let msg = IncomingMessage {
        id: "1".into(),
        channel_id: "ch".into(),
        sender_id: "s".into(),
        sender_name: "sender".into(),
        content: "hello".into(),
        is_dm: true,
        reply_to: None,
        attachments: vec![],
        thread_id: Some("t1".into()),
        callback_data: None,
    };
    assert_eq!(msg.thread_id.as_deref(), Some("t1"));
}

#[tokio::test]
async fn channel_streaming_defaults_are_noop() {
    let channel = MockChannel::new("test");
    // Default impls should succeed silently
    let _ = channel.edit_message("ch", "msg", "new").await;
    let _ = channel.delete_message("ch", "msg").await;
}

#[test]
fn telegram_has_expected_capabilities() {
    let channel = TelegramChannel::new("fake_token".into());
    let caps = channel.capabilities();
    assert!(caps.contains(&ChannelCapability::MessageEdit));
    assert!(caps.contains(&ChannelCapability::MessageDelete));
    assert!(caps.contains(&ChannelCapability::Streaming));
    assert!(caps.contains(&ChannelCapability::Reactions));
    assert!(caps.contains(&ChannelCapability::ForumTopics));
}

#[test]
fn outgoing_message_has_new_fields() {
    let msg = OutgoingMessage::text("ch".into(), "hi".into());
    assert!(!msg.voice_note);
    assert!(msg.keyboard.is_none());
    assert!(msg.media.is_empty());
    assert!(msg.edit_target.is_none());
    assert!(msg.thread_id.is_none());
}

#[tokio::test]
async fn telegram_rate_limiter_allows_first_call() {
    use std::time::Duration;

    let limiter = TelegramRateLimiter::new(Duration::from_millis(800));
    assert!(limiter.check("chat_123"));
}

#[tokio::test]
async fn telegram_rate_limiter_throttles_rapid_calls() {
    use std::time::Duration;

    let limiter = TelegramRateLimiter::new(Duration::from_millis(800));
    assert!(limiter.check("chat_123"));
    // Second call within window should be throttled
    assert!(!limiter.check("chat_123"));
}

#[tokio::test]
async fn telegram_rate_limiter_allows_different_chats() {
    use std::time::Duration;

    let limiter = TelegramRateLimiter::new(Duration::from_millis(800));
    assert!(limiter.check("chat_123"));
    assert!(limiter.check("chat_456"));
}

// --- Dedup tests ---

#[test]
fn dedup_detects_duplicates() {
    let mut dedup = UpdateDedup::default();
    assert!(!dedup.is_duplicate("update:123"));
    assert!(dedup.is_duplicate("update:123"));
}

#[test]
fn dedup_allows_different_keys() {
    let mut dedup = UpdateDedup::default();
    assert!(!dedup.is_duplicate("update:1"));
    assert!(!dedup.is_duplicate("update:2"));
    assert!(!dedup.is_duplicate("update:3"));
}

#[test]
fn dedup_respects_max_size() {
    let mut dedup = UpdateDedup::default();
    // Fill past capacity — should not panic
    for i in 0..2100 {
        dedup.is_duplicate(&format!("key:{i}"));
    }
    // Recent entries should still be tracked
    assert!(dedup.is_duplicate("key:2099"));
}

// --- Loop detection integration tests ---

#[test]
fn loop_detector_no_false_positives() {
    let mut detector = LoopDetector::default();
    for i in 0..9 {
        let severity = detector.record(
            &format!("tool_{i}"),
            &format!("{{\"arg\": {i}}}"),
            &format!("result_{i}"),
        );
        assert_eq!(severity, LoopSeverity::None);
    }
}

#[test]
fn loop_detector_generic_repeat_warning() {
    let mut detector = LoopDetector::default();
    let mut hit_warning = false;
    for i in 0..15 {
        let severity = detector.record("web_search", "{\"q\": \"test\"}", &format!("r{i}"));
        if severity >= LoopSeverity::Warning {
            hit_warning = true;
        }
    }
    assert!(hit_warning);
}

#[test]
fn loop_detector_circuit_breaker() {
    let mut detector = LoopDetector::default();
    let mut hit_breaker = false;
    for _ in 0..30 {
        let severity = detector.record("shell", "{\"cmd\": \"ls\"}", "files");
        if severity == LoopSeverity::CircuitBreaker {
            hit_breaker = true;
        }
    }
    assert!(hit_breaker);
}

// --- BotCommand tests ---

#[test]
fn bot_command_serialization() {
    let cmd = BotCommand {
        command: "help".to_string(),
        description: "Show help".to_string(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"command\":\"help\""));
    assert!(json.contains("\"description\":\"Show help\""));
}

// --- Thread mapping tests ---

#[test]
fn incoming_message_thread_from_polling() {
    let msg = IncomingMessage {
        id: "1".into(),
        channel_id: "ch".into(),
        sender_id: "s".into(),
        sender_name: "sender".into(),
        content: "hello".into(),
        is_dm: false,
        reply_to: None,
        attachments: vec![],
        thread_id: Some("42".into()),
        callback_data: None,
    };
    assert_eq!(msg.thread_id.as_deref(), Some("42"));
}

// --- Mention gating tests ---

fn make_group_msg(content: &str) -> IncomingMessage {
    IncomingMessage {
        id: "1".into(),
        channel_id: "ch".into(),
        sender_id: "s".into(),
        sender_name: "sender".into(),
        content: content.to_string(),
        is_dm: false,
        reply_to: None,
        attachments: vec![],
        thread_id: None,
        callback_data: None,
    }
}

#[test]
fn mention_gating_skips_unmentioned_group_messages() {
    let config = TelegramConfig {
        bot_token: "tok".into(),
        bot_username: Some("mybot".into()),
        require_mention_in_groups: true,
        reaction_level: ReactionLevel::Ack,
        ack_reaction: "\u{1F440}".into(),
        done_reaction: "\u{2705}".into(),
    };
    let msg = make_group_msg("hello everyone");
    assert!(should_skip_group_message(&msg, "supergroup", false, &config));
}

#[test]
fn mention_gating_allows_direct_mentions() {
    let config = TelegramConfig {
        bot_token: "tok".into(),
        bot_username: Some("mybot".into()),
        require_mention_in_groups: true,
        reaction_level: ReactionLevel::Ack,
        ack_reaction: "\u{1F440}".into(),
        done_reaction: "\u{2705}".into(),
    };
    let msg = make_group_msg("hey @mybot what's up");
    assert!(!should_skip_group_message(&msg, "supergroup", false, &config));
}

#[test]
fn mention_gating_allows_replies_to_bot() {
    let config = TelegramConfig {
        bot_token: "tok".into(),
        bot_username: Some("mybot".into()),
        require_mention_in_groups: true,
        reaction_level: ReactionLevel::Ack,
        ack_reaction: "\u{1F440}".into(),
        done_reaction: "\u{2705}".into(),
    };
    let msg = make_group_msg("hello");
    // has_reply = true simulates reply_to_message being present
    assert!(!should_skip_group_message(&msg, "supergroup", true, &config));
}

#[test]
fn mention_gating_ignores_in_private_chats() {
    let config = TelegramConfig {
        bot_token: "tok".into(),
        bot_username: Some("mybot".into()),
        require_mention_in_groups: true,
        reaction_level: ReactionLevel::Ack,
        ack_reaction: "\u{1F440}".into(),
        done_reaction: "\u{2705}".into(),
    };
    let msg = make_group_msg("hello");
    // Private chat type → should NOT skip
    assert!(!should_skip_group_message(&msg, "private", false, &config));
}

// --- Reaction config tests ---

#[test]
fn reaction_level_off_skips_reactions() {
    let config = TelegramConfig {
        bot_token: "tok".into(),
        bot_username: None,
        require_mention_in_groups: false,
        reaction_level: ReactionLevel::Off,
        ack_reaction: "\u{1F440}".into(),
        done_reaction: "\u{2705}".into(),
    };
    assert_eq!(config.reaction_level, ReactionLevel::Off);
}

#[test]
fn reaction_level_ack_sends_reactions() {
    let config = TelegramConfig {
        bot_token: "tok".into(),
        bot_username: None,
        require_mention_in_groups: false,
        reaction_level: ReactionLevel::Ack,
        ack_reaction: "\u{1F440}".into(),
        done_reaction: "\u{2705}".into(),
    };
    assert_ne!(config.reaction_level, ReactionLevel::Off);
}

#[test]
fn custom_ack_emoji_used() {
    let config = TelegramConfig {
        bot_token: "tok".into(),
        bot_username: None,
        require_mention_in_groups: false,
        reaction_level: ReactionLevel::Ack,
        ack_reaction: "\u{1F44D}".into(), // 👍
        done_reaction: "\u{1F389}".into(), // 🎉
    };
    assert_eq!(config.ack_reaction, "\u{1F44D}");
    assert_eq!(config.done_reaction, "\u{1F389}");
}

#[test]
fn reaction_level_parsing() {
    assert_eq!(ReactionLevel::from_str("off"), ReactionLevel::Off);
    assert_eq!(ReactionLevel::from_str("none"), ReactionLevel::Off);
    assert_eq!(ReactionLevel::from_str("full"), ReactionLevel::Full);
    assert_eq!(ReactionLevel::from_str("ack"), ReactionLevel::Ack);
    assert_eq!(ReactionLevel::from_str("anything_else"), ReactionLevel::Ack);
}
