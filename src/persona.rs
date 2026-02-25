//! Persona configuration and management
//!
//! Implements the persona.json specification for portable digital entity identity.
//! See: <https://persona.omni.dev>

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::tools::{ToolPolicy, ToolPolicyConfig};

/// A persona defines the identity of a digital entity
///
/// Follows the persona.json v1 specification.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Persona {
    /// Schema URL (optional, for validation)
    #[serde(rename = "$schema")]
    pub schema: Option<String>,

    /// Semantic version of this persona file
    pub version: String,

    /// Core identity (required)
    pub identity: Identity,

    /// Voice and audio configuration
    pub voice: Option<Voice>,

    /// Behavior and communication style
    pub personality: Option<Personality>,

    /// Visual identity and assets
    pub branding: Option<Branding>,

    /// Permissions and tool policies
    pub capabilities: Option<Capabilities>,

    // Beacon-specific extensions (not in base spec)
    /// Memory configuration for session management
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Context configuration (life.json access)
    #[serde(default)]
    pub context: ContextConfig,

    /// Knowledge configuration (inline facts + pack references)
    #[serde(default)]
    pub knowledge: KnowledgeConfig,
}

/// Core identity of the entity
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    /// Unique identifier
    pub id: String,

    /// Display name
    pub name: String,

    /// Short descriptive phrase
    pub tagline: Option<String>,

    /// Emoji or icon identifier
    pub icon: Option<String>,

    /// Entity type classification
    #[serde(rename = "type")]
    pub entity_type: Option<EntityType>,

    /// Longer description
    pub description: Option<String>,

    /// Primary URL
    pub url: Option<String>,
}

/// Entity type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityType {
    Assistant,
    Brand,
    Bot,
    Character,
    Mascot,
    Service,
}

/// Voice and audio configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Voice {
    /// Wake words for activation (first is primary)
    #[serde(default)]
    pub wake_words: Vec<String>,

    /// Text-to-speech configuration
    pub tts: Option<TtsConfig>,

    /// Speech-to-text configuration
    pub stt: Option<SttConfig>,
}

/// TTS configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsConfig {
    /// TTS provider
    pub provider: Option<String>,

    /// Voice identifier
    pub voice: Option<String>,

    /// Speech rate multiplier
    #[serde(default = "default_tts_speed")]
    pub speed: f32,

    /// Pitch adjustment
    pub pitch: Option<f32>,
}

/// STT configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SttConfig {
    /// STT provider
    pub provider: Option<String>,

    /// Model identifier
    pub model: Option<String>,

    /// Primary language (BCP 47 code)
    pub language: Option<String>,
}

/// Behavior and communication style
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Personality {
    /// Base system prompt
    pub system_prompt: Option<String>,

    /// Default communication tone
    pub tone: Option<String>,

    /// Default response length
    pub verbosity: Option<String>,

    /// Areas of specialized knowledge
    #[serde(default)]
    pub expertise: Vec<String>,

    /// Personality traits
    #[serde(default)]
    pub traits: Vec<String>,

    /// Mode-specific guidelines
    pub guidelines: Option<PersonalityGuidelines>,
}

/// Mode-specific behavior guidelines
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalityGuidelines {
    /// Additional instructions for voice interactions
    pub voice: Option<String>,

    /// Additional instructions for text interactions
    pub text: Option<String>,
}

/// Visual identity and brand assets
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branding {
    /// Brand color palette
    pub colors: Option<BrandColors>,

    /// Visual assets
    pub assets: Option<BrandAssets>,

    /// Marketing description
    pub description: Option<String>,
}

/// Brand color palette
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandColors {
    /// Primary brand color (hex)
    pub primary: Option<String>,

    /// Accent color (hex)
    pub accent: Option<String>,

    /// Background color (hex)
    pub background: Option<String>,
}

/// Visual assets
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandAssets {
    /// Avatar image URL
    pub avatar: Option<String>,

    /// Logo image URL
    pub logo: Option<String>,

    /// Banner image URL
    pub banner: Option<String>,
}

/// Permissions and tool policies
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Tool profiles by channel
    pub tools: Option<ToolPolicyConfig>,

    /// Global permission flags
    pub permissions: Option<CapabilityPermissions>,

    /// External integrations
    #[serde(default)]
    pub integrations: Vec<Integration>,
}

/// Global permission flags
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityPermissions {
    /// Can learn and remember facts
    pub can_learn: Option<bool>,

    /// Can browse the web
    pub can_access_web: Option<bool>,

    /// Can execute code
    pub can_execute_code: Option<bool>,

    /// Can access local files
    pub can_access_files: Option<bool>,
}

/// External integration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Integration {
    /// Integration type
    #[serde(rename = "type")]
    pub integration_type: String,

    /// Integration identifier
    pub id: String,

    /// Integration-specific configuration
    pub config: Option<serde_json::Value>,
}

/// Memory configuration for session management (Beacon extension)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConfig {
    /// Maximum context tokens to use
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,

    /// Maximum messages to retain in context
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,

    /// Pruning strategy
    #[serde(default = "default_pruning_strategy")]
    pub pruning_strategy: String,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: default_max_context_tokens(),
            max_messages: default_max_messages(),
            pruning_strategy: default_pruning_strategy(),
        }
    }
}

/// Context configuration for life.json access (Beacon extension)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextConfig {
    /// life.json slices this persona can read
    #[serde(default = "default_life_json_read")]
    pub life_json_read: Vec<String>,

    /// life.json slices this persona can write
    #[serde(default = "default_life_json_write")]
    pub life_json_write: Vec<String>,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            life_json_read: default_life_json_read(),
            life_json_write: default_life_json_write(),
        }
    }
}

/// Knowledge configuration for a persona
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeConfig {
    /// Inline knowledge chunks owned by this persona
    #[serde(default)]
    pub inline: Vec<KnowledgeChunk>,

    /// References to external knowledge packs on Manifold
    #[serde(default)]
    pub packs: Vec<KnowledgePackRef>,
}

/// A single knowledge chunk
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeChunk {
    /// Human-readable topic label
    pub topic: String,

    /// Machine-readable tags for selection
    #[serde(default)]
    pub tags: Vec<String>,

    /// Freeform knowledge content (markdown)
    pub content: String,

    /// Behavioral rules injected alongside this chunk
    #[serde(default)]
    pub rules: Vec<String>,

    /// Injection priority
    #[serde(default)]
    pub priority: KnowledgePriority,

    /// Pre-computed embedding vector for semantic selection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

/// When to inject a knowledge chunk
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KnowledgePriority {
    /// Inject every turn (core identity facts)
    Always,
    /// Inject when tags match user message
    #[default]
    Relevant,
}

/// Reference to an external knowledge pack on Manifold
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePackRef {
    /// Manifold artifact path: @{namespace}/knowledge/{artifact}
    #[serde(rename = "ref")]
    pub pack_ref: String,

    /// Semver version constraint
    pub version: Option<String>,

    /// Override priority for all chunks in this pack
    pub priority: Option<KnowledgePriority>,
}

/// Pre-computed embedding vectors for knowledge pack chunks
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackEmbeddings {
    /// Embedding model used to generate vectors
    pub model: String,

    /// Dimensionality of each vector
    pub dimensions: usize,

    /// Map from chunk index (as string) to embedding vector
    pub vectors: HashMap<String, Vec<f32>>,
}

/// A knowledge pack published to Manifold
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePack {
    /// Schema URL
    #[serde(rename = "$schema")]
    pub schema: Option<String>,

    /// Semver version
    pub version: String,

    /// Display name
    pub name: String,

    /// Description
    pub description: Option<String>,

    /// Pack-level tags (for marketplace search)
    #[serde(default)]
    pub tags: Vec<String>,

    /// Knowledge chunks
    pub chunks: Vec<KnowledgeChunk>,

    /// Pre-computed embeddings for chunks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<PackEmbeddings>,
}

// Default value functions

const fn default_max_context_tokens() -> usize {
    8000
}

const fn default_max_messages() -> usize {
    50
}

fn default_pruning_strategy() -> String {
    "fifo".to_string()
}

const fn default_tts_speed() -> f32 {
    1.0
}

fn default_life_json_read() -> Vec<String> {
    vec![
        "identity".to_string(),
        "preferences".to_string(),
        "calendar".to_string(),
    ]
}

fn default_life_json_write() -> Vec<String> {
    vec!["assistants".to_string()]
}

impl Default for Persona {
    fn default() -> Self {
        Self {
            schema: None,
            version: "1.0.0".to_string(),
            identity: Identity {
                id: "assistant".to_string(),
                name: "Assistant".to_string(),
                tagline: None,
                icon: None,
                entity_type: Some(EntityType::Assistant),
                description: None,
                url: None,
            },
            voice: None,
            personality: None,
            branding: None,
            capabilities: None,
            memory: MemoryConfig::default(),
            context: ContextConfig::default(),
            knowledge: KnowledgeConfig::default(),
        }
    }
}

// Convenience methods

impl Persona {
    /// Get the unique identifier
    #[must_use]
    pub fn id(&self) -> &str {
        &self.identity.id
    }

    /// Get the display name
    #[must_use]
    pub fn name(&self) -> &str {
        &self.identity.name
    }

    /// Get the primary wake word (first in array, if voice-enabled)
    #[must_use]
    pub fn wake_word(&self) -> Option<&str> {
        self.voice.as_ref()?.wake_words.first().map(String::as_str)
    }

    /// Get all wake words
    #[must_use]
    pub fn all_wake_words(&self) -> Vec<&str> {
        self.voice
            .as_ref()
            .map(|v| v.wake_words.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Get the system prompt
    #[must_use]
    pub fn system_prompt(&self) -> Option<&str> {
        self.personality.as_ref()?.system_prompt.as_deref()
    }

    /// Get the TTS voice identifier
    #[must_use]
    pub fn tts_voice(&self) -> Option<&str> {
        self.voice.as_ref()?.tts.as_ref()?.voice.as_deref()
    }

    /// Get the TTS speech rate
    #[must_use]
    pub fn tts_speed(&self) -> f32 {
        self.voice
            .as_ref()
            .and_then(|v| v.tts.as_ref())
            .map_or(1.0, |tts| tts.speed)
    }

    /// Get the STT model
    #[must_use]
    pub fn stt_model(&self) -> Option<&str> {
        self.voice.as_ref()?.stt.as_ref()?.model.as_deref()
    }

    /// Get the tool policy
    #[must_use]
    pub fn tool_policy(&self) -> ToolPolicy {
        self.capabilities
            .as_ref()
            .and_then(|c| c.tools.as_ref())
            .map_or_else(ToolPolicy::default_policy, ToolPolicy::new)
    }

    /// Get the primary brand color
    #[must_use]
    pub fn primary_color(&self) -> Option<&str> {
        self.branding.as_ref()?.colors.as_ref()?.primary.as_deref()
    }

    /// Get the accent color
    #[must_use]
    pub fn accent_color(&self) -> Option<&str> {
        self.branding.as_ref()?.colors.as_ref()?.accent.as_deref()
    }

    /// Check if this persona is voice-enabled
    #[must_use]
    pub fn is_voice_enabled(&self) -> bool {
        self.voice.as_ref().is_some_and(|v| !v.wake_words.is_empty())
    }

    /// Get inline knowledge chunks
    #[must_use]
    pub fn knowledge_chunks(&self) -> &[KnowledgeChunk] {
        &self.knowledge.inline
    }

    /// Get knowledge pack references
    #[must_use]
    pub fn knowledge_packs(&self) -> &[KnowledgePackRef] {
        &self.knowledge.packs
    }

    /// Check if this persona has any knowledge configured
    #[must_use]
    pub fn has_knowledge(&self) -> bool {
        !self.knowledge.inline.is_empty() || !self.knowledge.packs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_persona_has_assistant_identity() {
        let p = Persona::default();
        assert_eq!(p.id(), "assistant");
        assert_eq!(p.name(), "Assistant");
        assert_eq!(p.identity.entity_type, Some(EntityType::Assistant));
        assert_eq!(p.version, "1.0.0");
    }

    #[test]
    fn default_persona_has_no_voice() {
        let p = Persona::default();
        assert_eq!(p.wake_word(), None);
        assert!(!p.is_voice_enabled());
        assert_eq!(p.tts_voice(), None);
        assert_eq!(p.system_prompt(), None);
    }
}
