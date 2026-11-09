/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

//! Transcription with the Vercel AI Gateway cloud models in one request, to
//! whichever gateway model is picked in settings. The live model streams
//! (see `vercel_live_transcription.rs`); when it has no live result, and for
//! file jobs, it runs this way with the batch model, since streaming models
//! may not take one-shot requests.

use crate::audio_toolkit::{encode_wav, OutputLanguageEvidence};
use crate::cloud_live_transcription::LiveText;
use crate::language_detection::{LanguageDecision, LanguageSource};
use crate::llm_client::{report_reqwest_error, shared_client};
use crate::managers::model::{supported_code, WHISPER_LANGUAGES};
use crate::managers::transcription::{post_process_transcription_text, Transcription};
use crate::settings::{get_settings, AppSettings};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use log::info;
use reqwest::header::HeaderValue;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::AppHandle;

const TRANSCRIPTION_URL: &str = "https://ai-gateway.vercel.sh/v4/ai/transcription-model";
const MODELS_URL: &str = "https://ai-gateway.vercel.sh/v1/models";
/// The tag the model list gives models that stream over the WebSocket.
const STREAMING_TAG: &str = "websocket-transcription";
/// The gateway wire versions the AI SDK sends, which these requests follow.
pub(crate) const PROTOCOL_VERSION: &str = "0.0.1";
pub(crate) const SPECIFICATION_VERSION: &str = "4";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MODELS_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
    language: Option<String>,
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    tags: Option<Vec<String>>,
}

/// The AI Gateway API key.
pub(crate) fn api_key(settings: &AppSettings) -> Result<String> {
    let api_key = settings
        .vercel_ai_gateway_api_key
        .expose()
        .trim()
        .to_string();
    if api_key.is_empty() {
        return Err(anyhow!(
            "No AI Gateway API key is set. Add one in the model's settings."
        ));
    }
    Ok(api_key)
}

/// The message of a gateway error body, or the body itself.
pub(crate) fn error_message(body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    parsed
        .as_ref()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .or_else(|| v.get("error"))
        })
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string())
}

/// The languages both gateway models are offered with (their rows in
/// `CLOUD_MODELS`).
fn supported_languages() -> Vec<String> {
    WHISPER_LANGUAGES.iter().map(|c| c.to_string()).collect()
}

/// The provider option that asks `model` for `language`, under the model's
/// provider (its id's prefix), or `None` for a provider whose option isn't
/// known; that model detects the language itself. The names are the AI SDK
/// providers', which the gateway passes options on to.
fn language_options(model: &str, language: &str, streaming: bool) -> Option<Value> {
    let provider = model.split('/').next()?;
    let options = match provider {
        // gpt-transcribe takes a list over REST; streaming takes `language`.
        "openai" if !streaming && model == "openai/gpt-transcribe" => {
            json!({ "languages": [language] })
        }
        "openai" | "mistral" => json!({ "language": language }),
        "elevenlabs" => json!({ "languageCode": language }),
        "google" => json!({ "languageCodes": [language] }),
        _ => return None,
    };
    let mut by_provider = serde_json::Map::new();
    by_provider.insert(provider.to_string(), options);
    Some(Value::Object(by_provider))
}

/// What a request to the gateway asks for.
pub(crate) struct Request {
    pub model: String,
    /// The picked language, when it is sent.
    pub language: Option<String>,
    pub provider_options: Option<Value>,
}

impl Request {
    /// A stream to the live model, or a one-shot request to the batch model.
    pub(crate) fn from_settings(settings: &AppSettings, streaming: bool) -> Result<Self> {
        let model = if streaming {
            &settings.vercel_live_transcription_model
        } else {
            &settings.vercel_transcription_model
        }
        .trim()
        .to_string();
        if model.is_empty() {
            return Err(anyhow!(
                "No AI Gateway model is picked. Choose one in the model's settings."
            ));
        }
        // The id goes out in a header; one typed in might not fit in one.
        HeaderValue::from_str(&model).map_err(|_| {
            anyhow!("The AI Gateway model id '{model}' has characters that can't be sent")
        })?;
        // Every provider option takes ISO 639-1, so only two-letter codes go.
        let picked = (settings.selected_language != "auto")
            .then(|| supported_code(&settings.selected_language, &supported_languages()).cloned())
            .flatten()
            .filter(|code| code.len() == 2);
        let provider_options = picked
            .as_deref()
            .and_then(|language| language_options(&model, language, streaming));
        let language = picked.filter(|_| provider_options.is_some());
        Ok(Self {
            model,
            language,
            provider_options,
        })
    }
}

/// Handy's usual text cleanup of a gateway transcript, with the language
/// that was sent or, failing that, the one the model `detected`.
pub(crate) fn finish(
    text: String,
    detected: Option<String>,
    settings: &AppSettings,
    sent: Option<String>,
) -> Transcription {
    // Providers report e.g. "en-US" or "eng"; Handy's codes are the model's.
    let detected = detected.and_then(|code| supported_code(&code, &supported_languages()).cloned());
    let (output_language, source) = match (&sent, &detected) {
        (Some(language), _) => (
            OutputLanguageEvidence::UserSelected(language.clone()),
            LanguageSource::Selected,
        ),
        (None, Some(language)) => (
            OutputLanguageEvidence::ModelDetected(language.clone()),
            LanguageSource::Model,
        ),
        (None, None) => (OutputLanguageEvidence::Unknown, LanguageSource::Model),
    };
    // Custom words aren't sent, so they always get the fuzzy correction pass.
    let text = post_process_transcription_text(
        text,
        settings,
        false,
        &output_language,
        &supported_languages(),
    );
    Transcription {
        text,
        language: Some(LanguageDecision {
            language: sent.or(detected),
            source,
        }),
    }
}

/// A live session's transcript with Handy's usual text cleanup.
pub(crate) fn finish_live(app: &AppHandle, live: LiveText) -> Transcription {
    let settings = get_settings(app);
    let sent = Request::from_settings(&settings, true)
        .ok()
        .and_then(|request| request.language);
    finish(live.text, live.language, &settings, sent)
}

/// Transcribe 16 kHz mono `samples` with the picked batch gateway model,
/// then apply Handy's usual text cleanup (custom words, filler words,
/// Chinese script).
pub async fn transcribe(app: &AppHandle, samples: &[f32]) -> Result<Transcription> {
    let started = Instant::now();
    let settings = get_settings(app);
    let api_key = api_key(&settings)?;
    let request = Request::from_settings(&settings, false)?;

    let wav = encode_wav(samples)?;
    let mut body = json!({ "audio": BASE64.encode(wav), "mediaType": "audio/wav" });
    if let Some(options) = &request.provider_options {
        body["providerOptions"] = options.clone();
    }

    let client = shared_client().map_err(|e| anyhow!(e))?;
    let response = client
        .post(TRANSCRIPTION_URL)
        .bearer_auth(&api_key)
        .header("ai-gateway-protocol-version", PROTOCOL_VERSION)
        .header(
            "ai-transcription-model-specification-version",
            SPECIFICATION_VERSION,
        )
        .header("ai-model-id", &request.model)
        .header("ai-gateway-auth-method", "api-key")
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            anyhow!(report_reqwest_error(
                "AI Gateway transcription request failed",
                &e
            ))
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "AI Gateway transcription with {} failed (HTTP {}): {}",
            request.model,
            status.as_u16(),
            error_message(&body)
        ));
    }

    let parsed: TranscriptionResponse = response.json().await.map_err(|e| {
        anyhow!(report_reqwest_error(
            "Invalid AI Gateway transcription response",
            &e
        ))
    })?;

    info!(
        "AI Gateway transcription with {} completed in {:.2}s for {:.2}s of audio",
        request.model,
        started.elapsed().as_secs_f64(),
        samples.len() as f64 / 16_000.0
    );

    Ok(finish(
        parsed.text,
        parsed.language,
        &settings,
        request.language,
    ))
}

/// The ids of the gateway's transcription models; with `streaming`, only
/// those that stream. The list needs no key.
pub async fn fetch_models(streaming: bool) -> Result<Vec<String>> {
    let client = shared_client().map_err(|e| anyhow!(e))?;
    let response = client
        .get(MODELS_URL)
        .timeout(MODELS_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            anyhow!(report_reqwest_error(
                "AI Gateway model list request failed",
                &e
            ))
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "AI Gateway model list failed (HTTP {}): {}",
            status.as_u16(),
            error_message(&body)
        ));
    }

    let parsed: ModelList = response.json().await.map_err(|e| {
        anyhow!(report_reqwest_error(
            "Invalid AI Gateway model list response",
            &e
        ))
    })?;
    Ok(parsed
        .data
        .into_iter()
        .filter(|model| model.kind.as_deref() == Some("transcription"))
        .filter(|model| {
            !streaming
                || model
                    .tags
                    .as_ref()
                    .is_some_and(|tags| tags.iter().any(|tag| tag == STREAMING_TAG))
        })
        .map(|model| model.id)
        .collect())
}
