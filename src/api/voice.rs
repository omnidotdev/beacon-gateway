//! Voice API endpoints for speech-to-text and text-to-speech

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::ApiState;

/// Build voice router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/transcribe", post(transcribe))
        .route("/synthesize", post(synthesize))
        .route("/capabilities", axum::routing::get(capabilities))
        .with_state(state)
}

/// Voice capabilities response
#[derive(Debug, Serialize)]
pub struct VoiceCapabilities {
    pub stt_available: bool,
    pub tts_available: bool,
}

/// Get voice capabilities
async fn capabilities(State(state): State<Arc<ApiState>>) -> Json<VoiceCapabilities> {
    Json(VoiceCapabilities {
        stt_available: state.stt.is_some(),
        tts_available: state.tts.is_some(),
    })
}

/// Transcription response
#[derive(Debug, Serialize)]
pub struct TranscribeResponse {
    pub text: String,
}

/// Transcribe audio to text
///
/// Accepts audio in WAV format (audio/wav) or `WebM` format (audio/webm)
async fn transcribe(
    State(state): State<Arc<ApiState>>,
    body: Bytes,
) -> Result<Json<TranscribeResponse>, VoiceError> {
    let stt = state
        .stt
        .as_ref()
        .ok_or(VoiceError::NotConfigured("STT not configured"))?;

    if body.is_empty() {
        return Err(VoiceError::BadRequest("Empty audio data"));
    }

    // TODO: Support WebM/Opus input and convert to WAV
    // For now, expect WAV format directly
    let text = stt
        .transcribe(&body)
        .await
        .map_err(|e| VoiceError::TranscriptionFailed(e.to_string()))?;

    Ok(Json(TranscribeResponse { text }))
}

/// Synthesis request
#[derive(Debug, Deserialize)]
pub struct SynthesizeRequest {
    pub text: String,
}

/// Synthesize text to speech
///
/// Returns audio in MP3 format
async fn synthesize(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<SynthesizeRequest>,
) -> Result<Response, VoiceError> {
    let tts = state
        .tts
        .as_ref()
        .ok_or(VoiceError::NotConfigured("TTS not configured"))?;

    if request.text.is_empty() {
        return Err(VoiceError::BadRequest("Empty text"));
    }

    // TODO: Add text length limit to prevent abuse
    let audio = tts
        .synthesize(&request.text)
        .await
        .map_err(|e| VoiceError::SynthesisFailed(e.to_string()))?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "audio/mpeg")],
        audio,
    )
        .into_response())
}

/// Voice API errors
#[derive(Debug)]
pub enum VoiceError {
    NotConfigured(&'static str),
    BadRequest(&'static str),
    TranscriptionFailed(String),
    SynthesisFailed(String),
}

impl IntoResponse for VoiceError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorResponse {
            error: ErrorBody,
        }

        #[derive(Serialize)]
        struct ErrorBody {
            code: &'static str,
            message: String,
        }

        let (status, code, message) = match self {
            Self::NotConfigured(msg) => (StatusCode::SERVICE_UNAVAILABLE, "not_configured", msg.to_string()),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg.to_string()),
            Self::TranscriptionFailed(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "transcription_failed", msg),
            Self::SynthesisFailed(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "synthesis_failed", msg),
        };

        (status, Json(ErrorResponse { error: ErrorBody { code, message } })).into_response()
    }
}
