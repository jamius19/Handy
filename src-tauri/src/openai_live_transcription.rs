/*
 * Author: Jamius Siam
 * Since: 06/11/2026
 */

//! Live transcription with the OpenAI `gpt-live-transcribe` cloud model: a
//! Realtime WebSocket session fed while the recording goes and committed
//! when it stops, so only the last words are left to transcribe then.

use crate::cloud_live_transcription::{closed, Command, LiveResult, LiveText};
use crate::openai_transcription::{api_key, hints};
use crate::settings::get_settings;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use futures_util::{Sink, SinkExt, StreamExt};
use log::{debug, info};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{self, Message};

const REALTIME_URL: &str = "wss://api.openai.com/v1/realtime?intent=transcription";
const MODEL: &str = "gpt-live-transcribe";
/// The input rate OpenAI's Realtime examples use; Handy records at 16 kHz.
const INPUT_RATE: u32 = 24_000;
/// How long the final transcript may take once the recording stops.
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    item_id: Option<String>,
    delta: Option<String>,
    transcript: Option<String>,
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
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
    let (languages, keywords) = hints(&settings);

    let connect_started = Instant::now();
    let mut request = REALTIME_URL.into_client_request()?;
    request
        .headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {api_key}").parse()?);
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .context("Couldn't connect to the OpenAI Realtime API")?;
    debug!(
        "OpenAI Realtime session connected in {:?}",
        connect_started.elapsed()
    );
    let (mut sink, mut events) = socket.split();

    // Serializes as the API's own value, e.g. "medium".
    let delay = settings.openai_live_transcribe_delay;
    let mut transcription = json!({ "model": MODEL, "delay": delay });
    if !languages.is_empty() {
        transcription["languages"] = json!(languages);
    }
    if !keywords.is_empty() {
        transcription["keywords"] = json!(keywords);
    }
    send(
        &mut sink,
        json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "audio": {
                    "input": {
                        "format": { "type": "audio/pcm", "rate": INPUT_RATE },
                        "transcription": transcription,
                        // The model has no server VAD: Handy commits the turn.
                        "turn_detection": null
                    }
                }
            }
        }),
    )
    .await?;

    let mut upsampler = Upsampler::default();
    let mut so_far = String::new();
    let mut item_id: Option<String> = None;
    let mut finished_at: Option<Instant> = None;
    loop {
        tokio::select! {
            command = commands.recv(), if reply.is_none() => match command {
                Some(Command::Audio(pcm)) => append(&mut sink, &upsampler.push(&pcm)).await?,
                Some(Command::Finish(result)) => {
                    append(&mut sink, &upsampler.flush()).await?;
                    send(&mut sink, json!({ "type": "input_audio_buffer.commit" })).await?;
                    *reply = Some(result);
                    finished_at = Some(Instant::now());
                }
                None => return Ok(None),
            },
            event = events.next() => {
                let text = match event {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(frame))) => {
                        return Err(anyhow!("OpenAI closed the Realtime session: {frame:?}"))
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e).context("OpenAI Realtime session failed"),
                    None => return Err(anyhow!("OpenAI closed the Realtime session")),
                };
                let event: Event = serde_json::from_str(text.as_str())
                    .context("Invalid OpenAI Realtime event")?;
                let ours = item_id.is_some() && event.item_id == item_id;
                match event.kind.as_str() {
                    "input_audio_buffer.committed" => item_id = event.item_id,
                    "conversation.item.input_audio_transcription.delta" => {
                        if let Some(delta) = event.delta {
                            so_far.push_str(&delta);
                            on_text(&so_far);
                        }
                    }
                    "conversation.item.input_audio_transcription.completed" if ours => {
                        if let Some(finished_at) = finished_at {
                            info!(
                                "OpenAI live transcription finished {:.2}s after the recording stopped (delay {:?})",
                                finished_at.elapsed().as_secs_f64(),
                                delay
                            );
                        }
                        // gpt-live-transcribe reports no detected language.
                        return Ok(Some(LiveText {
                            text: event.transcript.unwrap_or_default(),
                            language: None,
                        }));
                    }
                    "conversation.item.input_audio_transcription.failed" if ours => {
                        return Err(anyhow!(
                            "OpenAI live transcription failed: {}",
                            event.error.map(|e| e.message).unwrap_or_default()
                        ));
                    }
                    "error" => {
                        return Err(anyhow!(
                            "OpenAI Realtime error: {}",
                            event.error.map(|e| e.message).unwrap_or_default()
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
                return Err(anyhow!("OpenAI live transcription timed out"));
            }
        }
    }
}

async fn send<S>(sink: &mut S, event: Value) -> Result<()>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    sink.send(Message::text(event.to_string()))
        .await
        .context("Couldn't send to the OpenAI Realtime API")
}

/// Sends PCM16 audio, if there is any.
async fn append<S>(sink: &mut S, pcm: &[i16]) -> Result<()>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    if pcm.is_empty() {
        return Ok(());
    }
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    send(
        sink,
        json!({ "type": "input_audio_buffer.append", "audio": BASE64.encode(bytes) }),
    )
    .await
}

/// 16 kHz to 24 kHz PCM16 by linear interpolation: three samples out for
/// every two in, across chunk boundaries.
#[derive(Default)]
pub(crate) struct Upsampler {
    pending: Vec<f32>,
}

impl Upsampler {
    pub(crate) fn push(&mut self, pcm: &[f32]) -> Vec<i16> {
        self.pending.extend_from_slice(pcm);
        let mut out = Vec::with_capacity(self.pending.len() * 3 / 2);
        // Each pair needs the sample after it to interpolate toward.
        let mut i = 0;
        while i + 2 < self.pending.len() {
            let (a, b, c) = (self.pending[i], self.pending[i + 1], self.pending[i + 2]);
            out.extend([a, a + (b - a) * 2.0 / 3.0, b + (c - b) / 3.0].map(to_pcm16));
            i += 2;
        }
        self.pending.drain(..i);
        out
    }

    /// The last one or two samples, with nothing after them.
    pub(crate) fn flush(&mut self) -> Vec<i16> {
        let out = match self.pending.as_slice() {
            [a, b] => vec![*a, a + (b - a) * 2.0 / 3.0, *b],
            [a] => vec![*a],
            _ => Vec::new(),
        };
        self.pending.clear();
        out.into_iter().map(to_pcm16).collect()
    }
}

/// The same conversion Handy saves WAV files with.
fn to_pcm16(sample: f32) -> i16 {
    (sample * i16::MAX as f32) as i16
}
