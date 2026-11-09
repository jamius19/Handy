/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

//! Transcription with the Deepgram `nova-3` cloud model. Dictation streams to
//! it while the recording goes (`deepgram_live_transcription.rs`); this is
//! the pre-recorded request that background jobs use, and that a dictation
//! falls back to when the stream has no result.

use crate::audio_toolkit::OutputLanguageEvidence;
use crate::cloud_live_transcription::LiveText;
use crate::language_detection::{LanguageDecision, LanguageSource};
use crate::llm_client::{report_reqwest_error, shared_client};
use crate::managers::transcription::{post_process_transcription_text, Transcription};
use crate::openai_transcription::encode_wav;
use crate::settings::{get_settings, AppSettings};
use anyhow::{anyhow, Result};
use log::info;
use reqwest::Url;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tauri::AppHandle;

const LISTEN_URL: &str = "https://api.deepgram.com/v1/listen";
pub(crate) const MODEL: &str = "nova-3";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The languages Nova-3 transcribes in multilingual mode, which are also the
/// only codes it takes as a `language_hint` (anything else is a 400).
const MULTILINGUAL_LANGUAGES: &[&str] = &[
    "af", "ar", "be", "bn", "bs", "bg", "ca", "zh", "hr", "cs", "da", "nl", "en", "et", "fi", "fr",
    "gl", "de", "el", "gu", "he", "hi", "hu", "id", "it", "ja", "kn", "ko", "lv", "lt", "mk", "ms",
    "mr", "no", "fa", "pl", "pt", "ro", "ru", "sr", "sk", "sl", "es", "sv", "tl", "ta", "te", "th",
    "tr", "uk", "vi", "xh", "zu",
];

/// Every language Nova-3 transcribes, as Handy's codes: the multilingual
/// ones, then those it only transcribes on their own.
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "af", "ar", "be", "bn", "bs", "bg", "ca", "zh", "hr", "cs", "da", "nl", "en", "et", "fi", "fr",
    "gl", "de", "el", "gu", "he", "hi", "hu", "id", "it", "ja", "kn", "ko", "lv", "lt", "mk", "ms",
    "mr", "no", "fa", "pl", "pt", "ro", "ru", "sr", "sk", "sl", "es", "sv", "tl", "ta", "te", "th",
    "tr", "uk", "vi", "xh", "zu", // monolingual only:
    "hy", "as", "ka", "kk", "mn", "ne", "ps", "pa", "ur",
];

/// Deepgram rejects a request whose keyterms add up to more than 500
/// tokens. Tokens aren't counted locally, so stay well under it.
const KEYTERM_TOKEN_BUDGET: usize = 400;

/// Nova-3's `language` for multilingual (code-switching) transcription.
const MULTI: &str = "multi";

/// What a request asks Nova-3 for, the same for a stream and a pre-recorded
/// request.
pub(crate) struct Request {
    /// A Nova-3 language code, or `multi`.
    pub language: String,
    /// Languages to favour in multilingual mode.
    pub hints: Vec<String>,
    /// Custom words, to recognize as spelled.
    pub keyterms: Vec<String>,
    /// Whether every custom word went out as a keyterm.
    pub all_custom_words_sent: bool,
}

impl Request {
    pub(crate) fn from_settings(settings: &AppSettings) -> Self {
        let (language, hints) = languages(settings);
        let (keyterms, all_custom_words_sent) = keyterms(settings);
        Self {
            language,
            hints,
            keyterms,
            all_custom_words_sent,
        }
    }

    /// The query parameters Nova-3 takes for this request, before any that
    /// only a stream or a pre-recorded request needs.
    pub(crate) fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![
            ("model", MODEL.to_string()),
            ("language", self.language.clone()),
            ("smart_format", "true".to_string()),
            ("punctuate", "true".to_string()),
        ];
        params.extend(self.hints.iter().map(|h| ("language_hint", h.clone())));
        params.extend(self.keyterms.iter().map(|k| ("keyterm", k.clone())));
        params
    }

    /// The language the transcript is in when the request fixed one.
    fn fixed_language(&self) -> Option<&str> {
        (self.language != MULTI).then_some(self.language.as_str())
    }

    pub(crate) fn is_multilingual(&self) -> bool {
        self.language == MULTI
    }
}

/// The `language` to ask for and, in multilingual mode, the hints.
fn languages(settings: &AppSettings) -> (String, Vec<String>) {
    let supported = |code: &str| SUPPORTED_LANGUAGES.contains(&code);
    // The language picked for the model wins; one Nova-3 doesn't know is
    // left for it to detect.
    if settings.selected_language != "auto" {
        if supported(&settings.selected_language) {
            return (settings.selected_language.clone(), Vec::new());
        }
        return (MULTI.to_string(), Vec::new());
    }
    // With Auto: someone who speaks one language Nova-3 knows gets its
    // monolingual model, which is the more accurate one. Otherwise it
    // switches between languages, favouring theirs.
    let mut spoken: Vec<&str> = settings
        .spoken_languages
        .iter()
        .map(String::as_str)
        .filter(|code| supported(code))
        .collect();
    spoken.dedup();
    match spoken.as_slice() {
        [only] => (only.to_string(), Vec::new()),
        _ => (
            MULTI.to_string(),
            spoken
                .into_iter()
                .filter(|code| MULTILINGUAL_LANGUAGES.contains(code))
                .map(str::to_string)
                .collect(),
        ),
    }
}

/// The custom words that fit the keyterm budget, and whether that was all
/// of them.
fn keyterms(settings: &AppSettings) -> (Vec<String>, bool) {
    let words: Vec<&str> = settings
        .custom_words
        .iter()
        .map(|word| word.trim())
        .filter(|word| !word.is_empty())
        .collect();
    let mut budget = KEYTERM_TOKEN_BUDGET;
    let mut keyterms = Vec::new();
    for word in &words {
        let cost = estimated_tokens(word);
        if cost > budget {
            break;
        }
        budget -= cost;
        keyterms.push(word.to_string());
    }
    let all = keyterms.len() == words.len();
    (keyterms, all)
}

/// A generous guess at a term's token count: one per three characters, and
/// at least one per word.
fn estimated_tokens(term: &str) -> usize {
    let chars = term.chars().count().div_ceil(3);
    chars.max(term.split_whitespace().count()).max(1)
}

/// The Deepgram API key.
pub(crate) fn api_key(settings: &AppSettings) -> Result<String> {
    let api_key = settings.deepgram_api_key.expose().trim().to_string();
    if api_key.is_empty() {
        return Err(anyhow!(
            "No Deepgram API key is set. Add one in the model's settings."
        ));
    }
    Ok(api_key)
}

/// `Authorization` for Deepgram: it takes `Token`, not `Bearer`.
pub(crate) fn authorization(api_key: &str) -> String {
    format!("Token {api_key}")
}

/// One alternative transcript of a channel.
#[derive(Deserialize, Default)]
pub(crate) struct Alternative {
    #[serde(default)]
    pub transcript: String,
    /// In multilingual mode, the languages heard, most words first.
    #[serde(default)]
    pub languages: Vec<String>,
}

#[derive(Deserialize)]
struct PrerecordedResponse {
    results: PrerecordedResults,
}

#[derive(Deserialize)]
struct PrerecordedResults {
    channels: Vec<PrerecordedChannel>,
}

#[derive(Deserialize)]
struct PrerecordedChannel {
    alternatives: Vec<Alternative>,
}

/// Deepgram's error bodies: `err_msg` on the older endpoints, `message`
/// on the newer ones.
#[derive(Deserialize)]
pub(crate) struct ErrorResponse {
    err_msg: Option<String>,
    message: Option<String>,
    description: Option<String>,
}

impl ErrorResponse {
    /// The message in an error `body`, or the body itself.
    pub(crate) fn message_of(body: &str) -> String {
        serde_json::from_str::<ErrorResponse>(body)
            .ok()
            .and_then(|e| e.err_msg.or(e.message).or(e.description))
            .unwrap_or_else(|| body.to_string())
    }
}

/// The one base language among `languages`, if they all agree.
pub(crate) fn single_language<'a>(languages: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut found: Option<&str> = None;
    for language in languages {
        let base = base_language(language);
        match found {
            None => found = Some(base),
            Some(other) if other == base => {}
            Some(_) => return None,
        }
    }
    found.map(str::to_string)
}

/// `pt-BR` → `pt`: Handy's codes have no region.
fn base_language(code: &str) -> &str {
    code.split('-').next().unwrap_or(code)
}

/// Transcribe 16 kHz mono `samples` with a pre-recorded request, then apply
/// Handy's usual text cleanup (custom words, filler words, Chinese script).
pub async fn transcribe(app: &AppHandle, samples: &[f32]) -> Result<Transcription> {
    let started = Instant::now();
    let settings = get_settings(app);
    let api_key = api_key(&settings)?;
    let request = Request::from_settings(&settings);
    let url = Url::parse_with_params(LISTEN_URL, request.params())?;

    let client = shared_client().map_err(|e| anyhow!(e))?;
    let response = client
        .post(url)
        .header(reqwest::header::AUTHORIZATION, authorization(&api_key))
        .header(reqwest::header::CONTENT_TYPE, "audio/wav")
        .body(encode_wav(samples)?)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            anyhow!(report_reqwest_error(
                "Deepgram transcription request failed",
                &e
            ))
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Deepgram transcription failed (HTTP {}): {}",
            status.as_u16(),
            ErrorResponse::message_of(&body)
        ));
    }

    let parsed: PrerecordedResponse = response.json().await.map_err(|e| {
        anyhow!(report_reqwest_error(
            "Invalid Deepgram transcription response",
            &e
        ))
    })?;
    let alternative = parsed
        .results
        .channels
        .into_iter()
        .next()
        .and_then(|channel| channel.alternatives.into_iter().next())
        .unwrap_or_default();
    let detected = single_language(alternative.languages.iter().map(String::as_str));

    info!(
        "Deepgram transcription completed in {:.2}s for {:.2}s of audio",
        started.elapsed().as_secs_f64(),
        samples.len() as f64 / 16_000.0
    );

    Ok(finish(
        alternative.transcript,
        detected,
        &settings,
        &request,
    ))
}

/// A live session's transcript with Handy's usual text cleanup.
pub(crate) fn finish_live(app: &AppHandle, live: LiveText) -> Transcription {
    let settings = get_settings(app);
    let request = Request::from_settings(&settings);
    finish(live.text, live.language, &settings, &request)
}

/// Handy's usual text cleanup of a Nova-3 transcript, with the language it
/// `detected` in multilingual mode, if any.
fn finish(
    text: String,
    detected: Option<String>,
    settings: &AppSettings,
    request: &Request,
) -> Transcription {
    let language = request.fixed_language().map(str::to_string).or(detected);
    let output_language = match (&language, request.fixed_language()) {
        (Some(language), Some(_)) if *language == settings.selected_language => {
            OutputLanguageEvidence::UserSelected(language.clone())
        }
        (Some(language), Some(_)) => OutputLanguageEvidence::ModelConstrained(language.clone()),
        (Some(language), None) => OutputLanguageEvidence::ModelDetected(language.clone()),
        (None, _) => OutputLanguageEvidence::Unknown,
    };
    let supported: Vec<String> = SUPPORTED_LANGUAGES.iter().map(|c| c.to_string()).collect();
    // Custom words sent as keyterms skip the fuzzy correction pass, as they
    // do for a local model that was prompted with them.
    let text = post_process_transcription_text(
        text,
        settings,
        !request.keyterms.is_empty() && request.all_custom_words_sent,
        &output_language,
        &supported,
    );
    let source = if request.fixed_language().is_some() && settings.selected_language != "auto" {
        LanguageSource::Selected
    } else {
        LanguageSource::Model
    };
    Transcription {
        text,
        language: Some(LanguageDecision { language, source }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(selected: &str, spoken: &[&str]) -> AppSettings {
        let mut settings = crate::settings::get_default_settings();
        settings.selected_language = selected.to_string();
        settings.spoken_languages = spoken.iter().map(|s| s.to_string()).collect();
        settings
    }

    #[test]
    fn a_picked_language_is_asked_for() {
        assert_eq!(languages(&settings("de", &["en"])), ("de".into(), vec![]));
    }

    #[test]
    fn a_picked_language_nova3_lacks_runs_multilingual() {
        assert_eq!(languages(&settings("yo", &[])), ("multi".into(), vec![]));
    }

    #[test]
    fn auto_with_one_spoken_language_runs_monolingual() {
        assert_eq!(languages(&settings("auto", &["ur"])), ("ur".into(), vec![]));
    }

    #[test]
    fn auto_with_several_languages_hints_the_multilingual_ones() {
        assert_eq!(
            languages(&settings("auto", &["en", "ur", "yo", "es"])),
            ("multi".into(), vec!["en".into(), "es".into()])
        );
    }

    #[test]
    fn keyterms_stop_at_the_budget() {
        let mut s = settings("auto", &[]);
        s.custom_words = vec![
            "Handy".into(),
            "  ".into(),
            "x".repeat(3 * KEYTERM_TOKEN_BUDGET),
        ];
        assert_eq!(keyterms(&s), (vec!["Handy".to_string()], false));
        s.custom_words.pop();
        assert_eq!(keyterms(&s), (vec!["Handy".to_string()], true));
    }

    #[test]
    fn one_language_only_when_all_agree() {
        assert_eq!(single_language(["en", "en-US"]), Some("en".into()));
        assert_eq!(single_language(["en", "es"]), None);
        assert_eq!(single_language([]), None);
    }

    #[test]
    fn error_messages_are_read_from_either_shape() {
        assert_eq!(
            ErrorResponse::message_of(
                r#"{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials."}"#
            ),
            "Invalid credentials."
        );
        assert_eq!(ErrorResponse::message_of(r#"{"message":"Bad"}"#), "Bad");
        assert_eq!(ErrorResponse::message_of("oops"), "oops");
    }
}
