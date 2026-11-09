/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

//! Live transcription with the Deepgram `nova-3` cloud model: a streaming
//! WebSocket fed raw PCM while the recording goes. Nova-3 sends interim text
//! for the live overlay and final segments as it settles them; when the
//! recording stops, `CloseStream` flushes the rest and the segments so far
//! are the transcript.

use crate::cloud_live_transcription::{closed, Command, LiveResult, LiveText};
use crate::deepgram_transcription::{
    api_key, authorization, single_language, Alternative, ErrorResponse, Request,
};
use crate::settings::get_settings;
use anyhow::{anyhow, Context, Result};
use futures_util::{Sink, SinkExt, StreamExt};
use log::{debug, info};
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::mpsc;
use tokio::time::{interval_at, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{self, Message};

const STREAM_URL: &str = "wss://api.deepgram.com/v1/listen";
/// Handy records 16 kHz mono; Nova-3 takes it as is.
const SAMPLE_RATE: &str = "16000";
/// Deepgram drops a stream that gets no audio for 10 seconds; it asks for a
/// KeepAlive every 3-5 seconds while there is none to send.
const KEEP_ALIVE: Duration = Duration::from_secs(4);
/// How long the final transcript may take once the recording stops.
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    is_final: bool,
    channel: Option<Channel>,
    description: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct Channel {
    #[serde(default)]
    alternatives: Vec<Alternative>,
}

/// The settled segments so far, and the languages they were in.
#[derive(Default)]
struct Segments {
    text: String,
    languages: Vec<String>,
}

impl Segments {
    fn push(&mut self, alternative: Alternative) {
        let transcript = alternative.transcript.trim();
        if transcript.is_empty() {
            return;
        }
        self.text = joined(&self.text, transcript);
        self.languages.extend(alternative.languages);
    }

    fn into_live_text(self) -> LiveText {
        let language = single_language(self.languages.iter().map(String::as_str));
        LiveText {
            text: self.text,
            language,
        }
    }
}

/// The session: `Ok(None)` when it is dropped before it finishes. Once the
/// recording stops, `reply` holds where its result goes.
pub(crate) async fn run(
    app: &AppHandle,
    mut commands: mpsc::UnboundedReceiver<Command>,
    reply: &mut Option<LiveResult>,
    on_text: impl Fn(&str),
) -> Result<Option<LiveText>> {
    let settings = get_settings(app);
    let api_key = api_key(&settings)?;
    let request = Request::from_settings(&settings);

    let mut params = request.params();
    params.extend([
        ("encoding", "linear16".to_string()),
        ("sample_rate", SAMPLE_RATE.to_string()),
        ("channels", "1".to_string()),
        ("interim_results", "true".to_string()),
    ]);
    // Deepgram's setting for code-switching; monolingual keeps its default.
    if request.is_multilingual() {
        params.push(("endpointing", "100".to_string()));
    }
    let url = Url::parse_with_params(STREAM_URL, params)?;

    let connect_started = Instant::now();
    let mut ws_request = url.as_str().into_client_request()?;
    ws_request
        .headers_mut()
        .insert(AUTHORIZATION, authorization(&api_key).parse()?);
    let (socket, _) = tokio_tungstenite::connect_async(ws_request)
        .await
        .map_err(connect_error)?;
    debug!(
        "Deepgram stream connected in {:?} (language {})",
        connect_started.elapsed(),
        request.language
    );
    let (mut sink, mut events) = socket.split();

    let mut segments = Segments::default();
    let mut finished_at: Option<Instant> = None;
    let mut keep_alive = interval_at(Instant::now() + KEEP_ALIVE, KEEP_ALIVE);
    loop {
        tokio::select! {
            command = commands.recv(), if reply.is_none() => match command {
                Some(Command::Audio(pcm)) => {
                    send_audio(&mut sink, &pcm).await?;
                    keep_alive.reset();
                }
                Some(Command::Finish(result)) => {
                    // Deepgram transcribes what it still holds, sends it,
                    // then Metadata, then closes.
                    send(&mut sink, json!({ "type": "CloseStream" })).await?;
                    *reply = Some(result);
                    finished_at = Some(Instant::now());
                }
                None => return Ok(None),
            },
            _ = keep_alive.tick(), if reply.is_none() => {
                send(&mut sink, json!({ "type": "KeepAlive" })).await?;
            }
            event = events.next() => {
                let text = match event {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(frame))) => {
                        if finished_at.is_some() {
                            return Ok(Some(done(segments, finished_at)));
                        }
                        return Err(anyhow!("Deepgram closed the stream: {frame:?}"));
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e).context("Deepgram stream failed"),
                    None if finished_at.is_some() => {
                        return Ok(Some(done(segments, finished_at)));
                    }
                    None => return Err(anyhow!("Deepgram closed the stream")),
                };
                let event: Event = serde_json::from_str(text.as_str())
                    .context("Invalid Deepgram stream event")?;
                match event.kind.as_str() {
                    "Results" => {
                        let alternative = event
                            .channel
                            .and_then(|c| c.alternatives.into_iter().next())
                            .unwrap_or_default();
                        if event.is_final {
                            segments.push(alternative);
                            on_text(&segments.text);
                        } else {
                            on_text(&joined(&segments.text, alternative.transcript.trim()));
                        }
                    }
                    // Sent last, once CloseStream has flushed every result.
                    "Metadata" if finished_at.is_some() => {
                        return Ok(Some(done(segments, finished_at)));
                    }
                    "Error" => {
                        return Err(anyhow!(
                            "Deepgram stream error: {}",
                            event.description.or(event.message).unwrap_or_default()
                        ));
                    }
                    _ => {}
                }
            }
            // Nobody waits for the result any more (the dictation was cancelled).
            _ = closed(reply) => return Ok(None),
            _ = tokio::time::sleep_until(
                finished_at.unwrap_or_else(Instant::now) + FINISH_TIMEOUT
            ), if finished_at.is_some() => {
                return Err(anyhow!("Deepgram live transcription timed out"));
            }
        }
    }
}

fn done(segments: Segments, finished_at: Option<Instant>) -> LiveText {
    if let Some(finished_at) = finished_at {
        info!(
            "Deepgram live transcription finished {:.2}s after the recording stopped",
            finished_at.elapsed().as_secs_f64()
        );
    }
    segments.into_live_text()
}

/// Why the stream didn't open: Deepgram answers the upgrade with an HTTP
/// error (401 for a bad key, 400 for a bad parameter) and a JSON body.
fn connect_error(error: tungstenite::Error) -> anyhow::Error {
    match error {
        tungstenite::Error::Http(response) => {
            let body = response
                .body()
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_default();
            anyhow!(
                "Couldn't connect to Deepgram (HTTP {}): {}",
                response.status().as_u16(),
                ErrorResponse::message_of(&body)
            )
        }
        e => anyhow!(e).context("Couldn't connect to Deepgram"),
    }
}

async fn send<S>(sink: &mut S, event: serde_json::Value) -> Result<()>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    sink.send(Message::text(event.to_string()))
        .await
        .context("Couldn't send to Deepgram")
}

/// Sends 16 kHz mono audio as linear16, if there is any.
async fn send_audio<S>(sink: &mut S, pcm: &[f32]) -> Result<()>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    if pcm.is_empty() {
        return Ok(());
    }
    // The same conversion Handy saves WAV files with.
    let bytes: Vec<u8> = pcm
        .iter()
        .flat_map(|s| ((s * i16::MAX as f32) as i16).to_le_bytes())
        .collect();
    sink.send(Message::binary(bytes))
        .await
        .context("Couldn't send audio to Deepgram")
}

/// `next` after `text`, with a space between unless either side is written
/// without spaces (Chinese, Japanese).
pub(crate) fn joined(text: &str, next: &str) -> String {
    let (Some(last), Some(first)) = (text.chars().last(), next.chars().next()) else {
        return format!("{text}{next}");
    };
    if is_unspaced(last) || is_unspaced(first) {
        format!("{text}{next}")
    } else {
        format!("{text} {next}")
    }
}

/// CJK punctuation, kana, ideographs and full-width forms.
fn is_unspaced(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{30FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FF00}'..='\u{FFEF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alternative(transcript: &str, languages: &[&str]) -> Alternative {
        Alternative {
            transcript: transcript.to_string(),
            languages: languages.iter().map(|l| l.to_string()).collect(),
        }
    }

    #[test]
    fn segments_join_with_spaces_except_between_cjk() {
        assert_eq!(joined("", "Hello"), "Hello");
        assert_eq!(joined("Hello", "world."), "Hello world.");
        assert_eq!(joined("今日は", "いい天気。"), "今日はいい天気。");
        assert_eq!(joined("Hello", ""), "Hello");
    }

    #[test]
    fn segments_report_one_language_only_if_all_agree() {
        let mut segments = Segments::default();
        segments.push(alternative("Hola", &["es"]));
        segments.push(alternative("  ", &["en"]));
        segments.push(alternative("amigo", &["es"]));
        let live = segments.into_live_text();
        assert_eq!(live.text, "Hola amigo");
        assert_eq!(live.language.as_deref(), Some("es"));

        let mut segments = Segments::default();
        segments.push(alternative("Hola", &["es"]));
        segments.push(alternative("friend", &["en"]));
        assert_eq!(segments.into_live_text().language, None);
    }

    #[test]
    fn results_events_parse() {
        let event: Event = serde_json::from_str(
            r#"{"type":"Results","channel_index":[0,1],"is_final":true,"speech_final":true,
                "channel":{"alternatives":[{"transcript":"Hi there.","confidence":0.99,
                "words":[],"languages":["en"]}]}}"#,
        )
        .unwrap();
        assert_eq!(event.kind, "Results");
        assert!(event.is_final);
        let alternative = event.channel.unwrap().alternatives.remove(0);
        assert_eq!(alternative.transcript, "Hi there.");
        assert_eq!(alternative.languages, vec!["en".to_string()]);
    }
}
