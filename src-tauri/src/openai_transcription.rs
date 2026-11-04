/*
 * Author: Jamius Siam
 * Since: 04/11/2026
 */

//! Transcription with the OpenAI `gpt-transcribe` cloud model, used for
//! dictation and background jobs whenever it is the selected model.

use crate::audio_toolkit::OutputLanguageEvidence;
use crate::language_detection::{LanguageDecision, LanguageSource};
use crate::llm_client::{report_reqwest_error, shared_client};
use crate::managers::transcription::{post_process_transcription_text, Transcription};
use crate::settings::get_settings;
use anyhow::{anyhow, Result};
use hound::{WavSpec, WavWriter};
use log::info;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use std::io::Cursor;
use std::time::{Duration, Instant};
use tauri::AppHandle;

const TRANSCRIPTIONS_URL: &str = "https://api.openai.com/v1/audio/transcriptions";
const MODEL: &str = "gpt-transcribe";
/// The post-processing provider whose API key is reused.
const OPENAI_PROVIDER_ID: &str = "openai";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
    #[serde(default)]
    languages: Vec<DetectedLanguage>,
}

#[derive(Deserialize)]
struct DetectedLanguage {
    code: String,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

/// Transcribe 16 kHz mono `samples` with OpenAI, then apply Handy's usual
/// text cleanup (custom words, filler words, Chinese script).
pub async fn transcribe(app: &AppHandle, samples: &[f32]) -> Result<Transcription> {
    let started = Instant::now();
    let settings = get_settings(app);

    let api_key = settings
        .post_process_api_keys
        .get(OPENAI_PROVIDER_ID)
        .map(|key| key.trim().to_string())
        .unwrap_or_default();
    if api_key.is_empty() {
        return Err(anyhow!(
            "No OpenAI API key is set. Add one in the model's settings."
        ));
    }

    // The language picked for the model wins; with Auto, hints for every
    // language the person speaks, e.g. ["en", "bn"].
    let languages: Vec<String> = if settings.selected_language != "auto" {
        vec![settings.selected_language.clone()]
    } else {
        settings.spoken_languages.clone()
    };
    // OpenAI rejects keywords containing '<', '>' or line breaks.
    let keywords: Vec<String> = settings
        .custom_words
        .iter()
        .map(|word| word.trim())
        .filter(|word| !word.is_empty() && !word.contains(['<', '>', '\r', '\n']))
        .map(str::to_string)
        .collect();

    let wav = encode_wav(samples)?;
    let file = Part::bytes(wav)
        .file_name("audio.wav")
        .mime_str("audio/wav")?;
    let mut form = Form::new()
        .part("file", file)
        .text("model", MODEL)
        .text("response_format", "json");
    for language in &languages {
        form = form.text("languages[]", language.clone());
    }
    for keyword in &keywords {
        form = form.text("keywords[]", keyword.clone());
    }

    let client = shared_client().map_err(|e| anyhow!(e))?;
    let response = client
        .post(TRANSCRIPTIONS_URL)
        .bearer_auth(&api_key)
        .multipart(form)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            anyhow!(report_reqwest_error(
                "OpenAI transcription request failed",
                &e
            ))
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<ErrorResponse>(&body)
            .map(|e| e.error.message)
            .unwrap_or(body);
        return Err(anyhow!(
            "OpenAI transcription failed (HTTP {}): {}",
            status.as_u16(),
            message
        ));
    }

    let parsed: TranscriptionResponse = response.json().await.map_err(|e| {
        anyhow!(report_reqwest_error(
            "Invalid OpenAI transcription response",
            &e
        ))
    })?;

    // Only a single detected language is firm enough to steer cleanup.
    let detected = match parsed.languages.as_slice() {
        [only] => Some(only.code.clone()),
        _ => None,
    };
    let output_language = detected
        .clone()
        .map(OutputLanguageEvidence::ModelDetected)
        .unwrap_or(OutputLanguageEvidence::Unknown);
    // Custom words sent as keywords skip the fuzzy correction pass, as they do
    // for a local model that was prompted with them.
    let text = post_process_transcription_text(
        parsed.text,
        &settings,
        !keywords.is_empty(),
        &output_language,
        &languages,
    );

    info!(
        "OpenAI transcription completed in {:.2}s for {:.2}s of audio",
        started.elapsed().as_secs_f64(),
        samples.len() as f64 / 16_000.0
    );

    Ok(Transcription {
        text,
        language: Some(LanguageDecision {
            language: detected,
            source: LanguageSource::Model,
        }),
    })
}

/// 16-bit PCM WAV bytes, the same format Handy saves recordings in.
fn encode_wav(samples: &[f32]) -> Result<Vec<u8>> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut cursor, spec)?;
    for sample in samples {
        writer.write_sample((sample * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(cursor.into_inner())
}
