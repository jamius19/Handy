/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

//! Transcription with the OpenRouter cloud model: the recording goes in one
//! request to whichever OpenRouter transcription model is picked in settings.
//! OpenRouter has no streaming transcription, so this model never runs live.

use crate::audio_toolkit::{encode_wav, OutputLanguageEvidence};
use crate::cloud_live_transcription::LiveText;
use crate::language_detection::{LanguageDecision, LanguageSource};
use crate::llm_client::{app_headers, report_reqwest_error, shared_client};
use crate::managers::model::{supported_code, WHISPER_LANGUAGES};
use crate::managers::transcription::{post_process_transcription_text, Transcription};
use crate::settings::{get_settings, AppSettings};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use log::info;
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};
use tauri::AppHandle;

const TRANSCRIPTIONS_URL: &str = "https://openrouter.ai/api/v1/audio/transcriptions";
/// Only the models that take `/audio/transcriptions` requests.
const MODELS_URL: &str = "https://openrouter.ai/api/v1/models?output_modalities=transcription";
/// The post-processing provider whose API key is reused.
const OPENROUTER_PROVIDER_ID: &str = "openrouter";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

/// The OpenRouter API key, shared with the OpenRouter post-processing provider.
fn api_key(settings: &AppSettings) -> Option<String> {
    settings
        .post_process_api_keys
        .get(OPENROUTER_PROVIDER_ID)
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

/// The message of an OpenRouter error body, or the body itself.
fn error_message(body: String) -> String {
    serde_json::from_str::<ErrorResponse>(&body)
        .map(|e| e.error.message)
        .unwrap_or(body)
}

/// The languages the OpenRouter model is offered with (its row in
/// `CLOUD_MODELS`).
fn supported_languages() -> Vec<String> {
    WHISPER_LANGUAGES.iter().map(|c| c.to_string()).collect()
}

/// The picked language as the model's code, or `None` with Auto or a
/// language it doesn't know (picked for another model), which it then
/// detects itself.
fn picked_language(settings: &AppSettings, supported: &[String]) -> Option<String> {
    if settings.selected_language == "auto" {
        return None;
    }
    supported_code(&settings.selected_language, supported).cloned()
}

/// Handy's usual text cleanup of an OpenRouter transcript, with the
/// language that was sent, if any.
fn finish(text: String, settings: &AppSettings, supported: &[String]) -> Transcription {
    let picked = picked_language(settings, supported);
    let (output_language, source) = match &picked {
        Some(language) => (
            OutputLanguageEvidence::UserSelected(language.clone()),
            LanguageSource::Selected,
        ),
        None => (OutputLanguageEvidence::Unknown, LanguageSource::Model),
    };
    // OpenRouter ignores `prompt`, so custom words always get the fuzzy
    // correction pass.
    let text = post_process_transcription_text(text, settings, false, &output_language, supported);
    Transcription {
        text,
        language: Some(LanguageDecision {
            language: picked,
            source,
        }),
    }
}

/// Unused in practice, since this model doesn't stream; it gets the same
/// cleanup as a one-request transcript.
pub(crate) fn finish_live(app: &AppHandle, live: LiveText) -> Transcription {
    let settings = get_settings(app);
    finish(live.text, &settings, &supported_languages())
}

/// Transcribe 16 kHz mono `samples` with the picked OpenRouter model, then
/// apply Handy's usual text cleanup (custom words, filler words, Chinese
/// script).
pub async fn transcribe(app: &AppHandle, samples: &[f32]) -> Result<Transcription> {
    let started = Instant::now();
    let settings = get_settings(app);
    let api_key = api_key(&settings)
        .ok_or_else(|| anyhow!("No OpenRouter API key is set. Add one in the model's settings."))?;
    let model = settings.openrouter_transcription_model.trim();
    if model.is_empty() {
        return Err(anyhow!(
            "No OpenRouter model is picked. Choose one in the model's settings."
        ));
    }
    let supported = supported_languages();

    let wav = encode_wav(samples)?;
    let mut body = json!({
        "model": model,
        "input_audio": { "data": BASE64.encode(wav), "format": "wav" },
        "response_format": "json",
    });
    // It takes one language; without one the model detects it.
    if let Some(language) = picked_language(&settings, &supported) {
        body["language"] = json!(language);
    }

    let client = shared_client().map_err(|e| anyhow!(e))?;
    let response = client
        .post(TRANSCRIPTIONS_URL)
        .headers(app_headers())
        .bearer_auth(&api_key)
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            anyhow!(report_reqwest_error(
                "OpenRouter transcription request failed",
                &e
            ))
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "OpenRouter transcription with {} failed (HTTP {}): {}",
            model,
            status.as_u16(),
            error_message(body)
        ));
    }

    let parsed: TranscriptionResponse = response.json().await.map_err(|e| {
        anyhow!(report_reqwest_error(
            "Invalid OpenRouter transcription response",
            &e
        ))
    })?;

    info!(
        "OpenRouter transcription with {} completed in {:.2}s for {:.2}s of audio",
        model,
        started.elapsed().as_secs_f64(),
        samples.len() as f64 / 16_000.0
    );

    Ok(finish(parsed.text, &settings, &supported))
}

/// The ids of OpenRouter's transcription models.
pub async fn fetch_models(app: &AppHandle) -> Result<Vec<String>> {
    let settings = get_settings(app);
    let client = shared_client().map_err(|e| anyhow!(e))?;
    let mut request = client
        .get(MODELS_URL)
        .headers(app_headers())
        .timeout(REQUEST_TIMEOUT);
    if let Some(api_key) = api_key(&settings) {
        request = request.bearer_auth(api_key);
    }
    let response = request.send().await.map_err(|e| {
        anyhow!(report_reqwest_error(
            "OpenRouter model list request failed",
            &e
        ))
    })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "OpenRouter model list failed (HTTP {}): {}",
            status.as_u16(),
            error_message(body)
        ));
    }

    let parsed: ModelList = response.json().await.map_err(|e| {
        anyhow!(report_reqwest_error(
            "Invalid OpenRouter model list response",
            &e
        ))
    })?;
    Ok(parsed.data.into_iter().map(|model| model.id).collect())
}
