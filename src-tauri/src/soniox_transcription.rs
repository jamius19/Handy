/*
 * Author: Jamius Siam
 * Since: 13/11/2026
 */

//! Transcription with the Soniox async model picked in settings (e.g.
//! `stt-async-v6`): the recording is uploaded, transcribed as a job and
//! fetched once the job completes. It runs dictation and background jobs for
//! the Soniox cloud model, and stands in for Soniox Live
//! (`soniox_live_transcription.rs`) when its stream has no result and for its
//! file jobs, since a real-time model can't run a job.

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::audio_toolkit::{encode_wav, OutputLanguageEvidence};
use crate::cloud_live_transcription::LiveText;
use crate::deepgram_transcription::single_language;
use crate::language_detection::{LanguageDecision, LanguageSource};
use crate::llm_client::{report_reqwest_error, shared_client};
use crate::managers::transcription::{post_process_transcription_text, Transcription};
use crate::settings::{get_settings, AppSettings, SonioxRegion};
use anyhow::{anyhow, Result};
use log::{info, warn};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use specta::Type;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::AppHandle;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// The model catalogue can be fetched right before a dictation's request, so
/// it gets less time.
const MODELS_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a job may take once created; longer audio gets as long as it
/// lasts.
const PROCESSING_TIMEOUT: Duration = Duration::from_secs(120);
/// Status polls start fast, since a dictation is usually done in a second or
/// two, then slow down for long file jobs.
const FIRST_POLL: Duration = Duration::from_millis(250);
const MAX_POLL: Duration = Duration::from_secs(2);
/// A job still processing can't be deleted, so the delete is retried this
/// often (doubling up to the most), until the job could have finished.
const CLEANUP_FIRST_RETRY: Duration = Duration::from_secs(2);
const CLEANUP_MAX_RETRY: Duration = Duration::from_secs(15);
/// Status and transcript requests try this many times when the connection
/// drops or Soniox answers 429 or 5xx, so one blip doesn't throw away a job.
const GET_ATTEMPTS: u32 = 3;
const GET_RETRY_DELAY: Duration = Duration::from_secs(1);

/// The REST API's address for `region`; a project's key works only on its
/// own region's.
fn api_base(region: SonioxRegion) -> &'static str {
    match region {
        SonioxRegion::Us => "https://api.soniox.com",
        SonioxRegion::Eu => "https://api.eu.soniox.com",
        SonioxRegion::Jp => "https://api.jp.soniox.com",
        SonioxRegion::In => "https://api.in.soniox.com",
    }
}

/// The real-time WebSocket's address for `region`.
pub(crate) fn stream_url(region: SonioxRegion) -> &'static str {
    match region {
        SonioxRegion::Us => "wss://stt-rt.soniox.com/transcribe-websocket",
        SonioxRegion::Eu => "wss://stt-rt.eu.soniox.com/transcribe-websocket",
        SonioxRegion::Jp => "wss://stt-rt.jp.soniox.com/transcribe-websocket",
        SonioxRegion::In => "wss://stt-rt.in.soniox.com/transcribe-websocket",
    }
}

/// Every language Soniox transcribes, the same for every current model.
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "af", "sq", "ar", "az", "eu", "be", "bn", "bs", "bg", "ca", "zh", "hr", "cs", "da", "nl", "en",
    "et", "fi", "fr", "gl", "de", "el", "gu", "he", "hi", "hu", "id", "it", "ja", "kn", "kk", "ko",
    "lv", "lt", "mk", "ms", "ml", "mr", "no", "fa", "pl", "pt", "pa", "ro", "ru", "sr", "sk", "sl",
    "es", "sw", "sv", "tl", "ta", "te", "th", "tr", "uk", "ur", "vi", "cy",
];

/// Soniox rejects a context over 8,000 tokens (about 10,000 characters).
/// Tokens aren't counted locally, so the custom words stay well under it.
const TERMS_CHAR_BUDGET: usize = 8_000;

/// What a request asks Soniox for, the same for a stream and a job.
pub(crate) struct Request {
    /// Languages to favour.
    pub language_hints: Vec<String>,
    /// Whether the hint is the one language the person picked, which a model
    /// that supports it is held to.
    pub picked: bool,
    /// Custom words, to recognize as spelled.
    pub terms: Vec<String>,
    /// Whether every custom word went out as a term.
    pub all_custom_words_sent: bool,
}

impl Request {
    pub(crate) fn from_settings(settings: &AppSettings) -> Self {
        let (language_hints, picked) = language_hints(settings);
        let (terms, all_custom_words_sent) = terms(settings);
        Self {
            language_hints,
            picked,
            terms,
            all_custom_words_sent,
        }
    }

    /// The configuration both APIs take for `model`, before any field only a
    /// stream or a job needs. `strict_supported` is whether `model` takes
    /// `language_hints_strict` (see [`supports_strict`]).
    pub(crate) fn config(&self, model: &str, strict_supported: bool) -> Value {
        let mut config = json!({
            "model": model,
            "enable_language_identification": true,
        });
        if !self.language_hints.is_empty() {
            config["language_hints"] = json!(self.language_hints);
        }
        if self.picked && strict_supported {
            config["language_hints_strict"] = json!(true);
        }
        if !self.terms.is_empty() {
            config["context"] = json!({ "terms": self.terms });
        }
        config
    }

    /// The language the person picked, which the transcript is taken to be in.
    fn fixed_language(&self) -> Option<&str> {
        match self.language_hints.as_slice() {
            [only] if self.picked => Some(only),
            _ => None,
        }
    }
}

/// The language hints to send, and whether they are the one language the
/// person picked.
fn language_hints(settings: &AppSettings) -> (Vec<String>, bool) {
    let supported = |code: &str| SUPPORTED_LANGUAGES.contains(&code);
    // The language picked for the model wins; one Soniox doesn't know is
    // left for it to detect.
    if settings.selected_language != "auto" {
        if supported(&settings.selected_language) {
            return (vec![settings.selected_language.clone()], true);
        }
        return (Vec::new(), false);
    }
    // With Auto, favour every language the person speaks. Soniox rejects a
    // hint given twice.
    let mut hints: Vec<String> = Vec::new();
    for code in &settings.spoken_languages {
        if supported(code) && !hints.contains(code) {
            hints.push(code.clone());
        }
    }
    (hints, false)
}

/// The custom words that fit the context budget, and whether that was all
/// of them.
fn terms(settings: &AppSettings) -> (Vec<String>, bool) {
    let words: Vec<&str> = settings
        .custom_words
        .iter()
        .map(|word| word.trim())
        .filter(|word| !word.is_empty())
        .collect();
    let mut budget = TERMS_CHAR_BUDGET;
    let mut terms = Vec::new();
    for word in &words {
        let cost = word.chars().count();
        if cost > budget {
            break;
        }
        budget -= cost;
        terms.push(word.to_string());
    }
    let all = terms.len() == words.len();
    (terms, all)
}

/// The Soniox API key.
pub(crate) fn api_key(settings: &AppSettings) -> Result<String> {
    let api_key = settings.soniox_api_key.expose().trim().to_string();
    if api_key.is_empty() {
        return Err(anyhow!(
            "No Soniox API key is set. Add one in the model's settings."
        ));
    }
    Ok(api_key)
}

/// The Soniox async model picked in settings.
fn model(settings: &AppSettings) -> Result<String> {
    let model = settings.soniox_transcription_model.trim();
    if model.is_empty() {
        return Err(anyhow!(
            "No Soniox model is picked. Pick one in the model's settings."
        ));
    }
    Ok(model.to_string())
}

#[derive(Deserialize)]
struct ModelList {
    models: Vec<CatalogueModel>,
}

/// A model in Soniox's catalogue (`GET /v1/models`).
#[derive(Deserialize)]
struct CatalogueModel {
    id: String,
    /// `real_time` for a stream, `async` for a job.
    transcription_mode: String,
    #[serde(default)]
    supports_language_hints_strict: bool,
}

/// Whether each model in the catalogue takes `language_hints_strict`, from
/// the last time it was fetched.
static STRICT_SUPPORT: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

fn known_strict_support(model: &str) -> Option<bool> {
    let cache = STRICT_SUPPORT.lock().unwrap_or_else(|e| e.into_inner());
    cache.as_ref()?.get(model).copied()
}

/// Soniox's model catalogue at `base`, which also refreshes what is known of
/// each model's support for `language_hints_strict`.
async fn fetch_catalogue(base: &str, api_key: &str) -> Result<Vec<CatalogueModel>> {
    let client = shared_client().map_err(|e| anyhow!(e))?;
    let request = client.get(format!("{base}/v1/models"));
    let list: ModelList = parse(
        send(request, api_key, "model list", MODELS_TIMEOUT).await?,
        "model list",
    )
    .await?;
    let support = list
        .models
        .iter()
        .map(|m| (m.id.clone(), m.supports_language_hints_strict))
        .collect();
    *STRICT_SUPPORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(support);
    Ok(list.models)
}

/// Fetches the catalogue in the background when a key is set, so a
/// dictation doesn't wait on it to check its model (see [`supports_strict`]).
pub fn warm_catalogue(app: &AppHandle) {
    let settings = get_settings(app);
    let Ok(api_key) = api_key(&settings) else {
        return;
    };
    let base = api_base(settings.soniox_region);
    tauri::async_runtime::spawn(async move {
        if let Err(e) = fetch_catalogue(base, &api_key).await {
            warn!("Couldn't load the Soniox model catalogue: {e:#}");
        }
    });
}

/// Soniox's model ids for the Soniox cloud models' pickers, by how they run.
#[derive(Serialize, Type)]
pub struct SonioxModels {
    /// Real-time models, which stream.
    pub live: Vec<String>,
    /// Async models, which run as a job.
    pub batch: Vec<String>,
}

pub async fn fetch_models(app: &AppHandle) -> Result<SonioxModels> {
    let settings = get_settings(app);
    let api_key = api_key(&settings)?;
    let catalogue = fetch_catalogue(api_base(settings.soniox_region), &api_key).await?;
    let (live, batch): (Vec<_>, Vec<_>) = catalogue
        .into_iter()
        .filter(|model| matches!(model.transcription_mode.as_str(), "real_time" | "async"))
        .partition(|model| model.transcription_mode == "real_time");
    Ok(SonioxModels {
        live: live.into_iter().map(|model| model.id).collect(),
        batch: batch.into_iter().map(|model| model.id).collect(),
    })
}

/// Whether `model` takes `language_hints_strict`, which Soniox rejects from
/// a model that doesn't. The catalogue is fetched when the model isn't known
/// yet; a model it doesn't list, or a failed fetch, counts as no.
pub(crate) async fn supports_strict(region: SonioxRegion, api_key: &str, model: &str) -> bool {
    if let Some(known) = known_strict_support(model) {
        return known;
    }
    if let Err(e) = fetch_catalogue(api_base(region), api_key).await {
        warn!("Couldn't check whether Soniox model {model} takes strict language hints: {e:#}");
        return false;
    }
    let mut cache = STRICT_SUPPORT.lock().unwrap_or_else(|e| e.into_inner());
    // Remember a model the catalogue doesn't list, so it isn't fetched again.
    *cache
        .get_or_insert_with(HashMap::new)
        .entry(model.to_string())
        .or_insert(false)
}

/// One token of a transcript: a word, part of one, or a space.
#[derive(Deserialize)]
pub(crate) struct Token {
    #[serde(default)]
    pub text: String,
    /// Only a stream's tokens can be provisional.
    #[serde(default)]
    pub is_final: bool,
    /// The language the token was heard in.
    pub language: Option<String>,
}

/// Soniox's error fields: `message` on the REST API, `error_message` on the
/// WebSocket.
#[derive(Deserialize, Default)]
pub(crate) struct ErrorResponse {
    error_type: Option<String>,
    message: Option<String>,
    error_message: Option<String>,
    request_id: Option<String>,
}

impl ErrorResponse {
    /// `error_type: message (request id)`, from whichever fields are set.
    pub(crate) fn describe(&self) -> String {
        let message = self
            .message
            .as_deref()
            .or(self.error_message.as_deref())
            .unwrap_or_default();
        let mut text = match &self.error_type {
            Some(kind) if message.is_empty() => kind.clone(),
            Some(kind) => format!("{kind}: {message}"),
            None => message.to_string(),
        };
        if let Some(id) = &self.request_id {
            text.push_str(&format!(" (request {id})"));
        }
        text
    }

    /// The error in a response `body`, or the body itself.
    fn message_of(body: &str) -> String {
        serde_json::from_str::<ErrorResponse>(body)
            .ok()
            .map(|e| e.describe())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| body.to_string())
    }
}

#[derive(Deserialize)]
struct Created {
    id: String,
}

#[derive(Deserialize)]
struct Job {
    status: String,
    error_type: Option<String>,
    error_message: Option<String>,
}

#[derive(Deserialize)]
struct Transcript {
    text: String,
    #[serde(default)]
    tokens: Vec<Token>,
}

/// The uploaded file and its job. Dropping it deletes both in the
/// background, also when the request is dropped part-way (a cancelled
/// dictation): Soniox keeps them for 30 days otherwise, and they count
/// against its 1,000-file and 2,000-transcription limits.
struct Uploaded {
    base: &'static str,
    api_key: String,
    file_id: String,
    transcription_id: Option<String>,
    /// How long the job may run, and so how long its delete is retried.
    processing_timeout: Duration,
}

impl Drop for Uploaded {
    fn drop(&mut self) {
        tauri::async_runtime::spawn(clean_up(
            self.base,
            std::mem::take(&mut self.api_key),
            std::mem::take(&mut self.file_id),
            self.transcription_id.take(),
            self.processing_timeout,
        ));
    }
}

/// Transcribe 16 kHz mono `samples` as a Soniox job, then apply Handy's usual
/// text cleanup (custom words, filler words, Chinese script).
pub async fn transcribe(app: &AppHandle, samples: &[f32]) -> Result<Transcription> {
    let started = Instant::now();
    let settings = get_settings(app);
    let api_key = api_key(&settings)?;
    let model = model(&settings)?;
    let request = Request::from_settings(&settings);
    let region = settings.soniox_region;
    let base = api_base(region);
    let strict_supported = request.picked && supports_strict(region, &api_key, &model).await;
    let client = shared_client().map_err(|e| anyhow!(e))?;
    let seconds = samples.len() as f64 / WHISPER_SAMPLE_RATE as f64;
    let processing_timeout = PROCESSING_TIMEOUT.max(Duration::from_secs_f64(seconds));

    let file = Part::bytes(encode_wav(samples)?)
        .file_name("audio.wav")
        .mime_str("audio/wav")?;
    let upload = client
        .post(format!("{base}/v1/files"))
        .multipart(Form::new().part("file", file));
    let file: Created = parse(
        send(upload, &api_key, "upload", REQUEST_TIMEOUT).await?,
        "upload",
    )
    .await?;
    let mut uploaded = Uploaded {
        base,
        api_key: api_key.clone(),
        file_id: file.id,
        transcription_id: None,
        processing_timeout,
    };

    let mut config = request.config(&model, strict_supported);
    config["file_id"] = json!(uploaded.file_id);
    let create = client
        .post(format!("{base}/v1/transcriptions"))
        .json(&config);
    let job: Created = parse(
        send(create, &api_key, "transcription", REQUEST_TIMEOUT).await?,
        "transcription",
    )
    .await?;
    uploaded.transcription_id = Some(job.id.clone());

    wait_until_completed(client, base, &api_key, &job.id, processing_timeout).await?;

    let url = format!("{base}/v1/transcriptions/{}/transcript", job.id);
    let transcript: Transcript = parse(
        get(client, &api_key, &url, "transcript").await?,
        "transcript",
    )
    .await?;
    drop(uploaded);

    let detected = single_language(
        transcript
            .tokens
            .iter()
            .filter_map(|t| t.language.as_deref()),
    );
    info!(
        "Soniox {model} transcription completed in {:.2}s for {seconds:.2}s of audio",
        started.elapsed().as_secs_f64()
    );
    Ok(finish(transcript.text, detected, &settings, &request))
}

/// Polls the job until it completes, fails or runs out of `timeout`.
async fn wait_until_completed(
    client: &Client,
    base: &str,
    api_key: &str,
    id: &str,
    timeout: Duration,
) -> Result<()> {
    let url = format!("{base}/v1/transcriptions/{id}");
    let deadline = Instant::now() + timeout;
    let mut delay = FIRST_POLL;
    loop {
        let job: Job = parse(get(client, api_key, &url, "status").await?, "status").await?;
        match job.status.as_str() {
            "completed" => return Ok(()),
            // The API reference says "error", the errors page "failed".
            "error" | "failed" => {
                let error = ErrorResponse {
                    error_type: job.error_type,
                    error_message: job.error_message,
                    ..Default::default()
                };
                return Err(anyhow!("Soniox transcription failed: {}", error.describe()));
            }
            _ => {}
        }
        if Instant::now() + delay > deadline {
            return Err(anyhow!(
                "Soniox transcription timed out after {}s",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 3 / 2).min(MAX_POLL);
    }
}

/// Sends `request` with the key, failing on a non-2xx answer.
async fn send(
    request: RequestBuilder,
    api_key: &str,
    what: &str,
    timeout: Duration,
) -> Result<Response> {
    let result = request.bearer_auth(api_key).timeout(timeout).send().await;
    checked(result, what).await
}

/// GETs `url` with the key, retrying a dropped connection, 429 and 5xx up to
/// [`GET_ATTEMPTS`] times.
async fn get(client: &Client, api_key: &str, url: &str, what: &str) -> Result<Response> {
    let mut attempt = 1;
    loop {
        let result = client
            .get(url)
            .bearer_auth(api_key)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
        let transient = match &result {
            Ok(response) => {
                let status = response.status();
                status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
            }
            Err(e) => e.is_timeout() || e.is_connect() || e.is_request(),
        };
        if !transient || attempt == GET_ATTEMPTS {
            return checked(result, what).await;
        }
        warn!("Soniox {what} request failed (attempt {attempt} of {GET_ATTEMPTS}); retrying");
        tokio::time::sleep(GET_RETRY_DELAY * attempt).await;
        attempt += 1;
    }
}

/// `result`'s response, or why it failed: a send error or a non-2xx answer.
async fn checked(result: reqwest::Result<Response>, what: &str) -> Result<Response> {
    let response = result.map_err(|e| {
        anyhow!(report_reqwest_error(
            &format!("Soniox {what} request failed"),
            &e
        ))
    })?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    Err(anyhow!(
        "Soniox {what} failed (HTTP {}): {}",
        status.as_u16(),
        ErrorResponse::message_of(&body)
    ))
}

async fn parse<T: DeserializeOwned>(response: Response, what: &str) -> Result<T> {
    response.json().await.map_err(|e| {
        anyhow!(report_reqwest_error(
            &format!("Invalid Soniox {what} response"),
            &e
        ))
    })
}

/// Deletes the file, then its job. A job that hasn't started yet fails once
/// its file is gone; one already processing can't be deleted (409) until it
/// finishes, so its delete is retried for as long as it may run.
async fn clean_up(
    base: &str,
    api_key: String,
    file_id: String,
    transcription_id: Option<String>,
    processing_timeout: Duration,
) {
    let Ok(client) = shared_client() else {
        return;
    };
    if let Err(e) = delete(client, &api_key, &format!("{base}/v1/files/{file_id}")).await {
        warn!("Couldn't delete Soniox file {file_id}: {e:#}");
    }
    let Some(id) = transcription_id else {
        return;
    };
    let url = format!("{base}/v1/transcriptions/{id}");
    let deadline = Instant::now() + processing_timeout;
    let mut delay = CLEANUP_FIRST_RETRY;
    loop {
        match delete(client, &api_key, &url).await {
            Ok(true) => return,
            Ok(false) if Instant::now() + delay <= deadline => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(CLEANUP_MAX_RETRY);
            }
            Ok(false) => break,
            Err(e) => {
                warn!("Couldn't delete Soniox transcription {id}: {e:#}");
                return;
            }
        }
    }
    warn!("Soniox transcription {id} was still processing, so it wasn't deleted");
}

/// `Ok(false)` while a job is still processing; one already gone counts as
/// deleted.
async fn delete(client: &Client, api_key: &str, url: &str) -> Result<bool> {
    let response = client
        .delete(url)
        .bearer_auth(api_key)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| anyhow!(report_reqwest_error("Soniox delete request failed", &e)))?;
    let status = response.status();
    if status.is_success() || status == StatusCode::NOT_FOUND {
        return Ok(true);
    }
    if status == StatusCode::CONFLICT {
        return Ok(false);
    }
    let body = response.text().await.unwrap_or_default();
    Err(anyhow!(
        "HTTP {}: {}",
        status.as_u16(),
        ErrorResponse::message_of(&body)
    ))
}

/// A live session's transcript with Handy's usual text cleanup.
pub(crate) fn finish_live(app: &AppHandle, live: LiveText) -> Transcription {
    let settings = get_settings(app);
    let request = Request::from_settings(&settings);
    finish(live.text, live.language, &settings, &request)
}

/// Handy's usual text cleanup of a Soniox transcript, with the language it
/// `detected`, if any.
fn finish(
    text: String,
    detected: Option<String>,
    settings: &AppSettings,
    request: &Request,
) -> Transcription {
    // Only a language the person picked is fixed.
    let fixed = request.fixed_language().map(str::to_string);
    let output_language = match (&fixed, &detected) {
        (Some(language), _) => OutputLanguageEvidence::UserSelected(language.clone()),
        (None, Some(language)) => OutputLanguageEvidence::ModelDetected(language.clone()),
        (None, None) => OutputLanguageEvidence::Unknown,
    };
    let source = if fixed.is_some() {
        LanguageSource::Selected
    } else {
        LanguageSource::Model
    };
    let language = fixed.or(detected);
    let supported: Vec<String> = SUPPORTED_LANGUAGES.iter().map(|c| c.to_string()).collect();
    // Custom words sent as terms skip the fuzzy correction pass, as they do
    // for a local model that was prompted with them.
    let text = post_process_transcription_text(
        text.trim().to_string(),
        settings,
        !request.terms.is_empty() && request.all_custom_words_sent,
        &output_language,
        &supported,
    );
    Transcription {
        text,
        language: Some(LanguageDecision { language, source }),
    }
}
