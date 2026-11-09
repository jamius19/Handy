/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

//! Live transcription with the Vercel AI Gateway Live cloud model: the
//! gateway's transcription-stream WebSocket, fed raw PCM while the recording
//! goes. After a start frame, audio rides binary frames; the server sends
//! partial and final transcript parts for the live overlay, and once
//! `audio-done` is sent, a `finish` part with the whole transcript. The
//! envelope is the AI SDK's (`@ai-sdk/provider-utils`
//! `transcription-stream-envelope`).

use crate::cloud_live_transcription::{closed, Command, LiveResult, LiveText};
use crate::deepgram_live_transcription::joined;
use crate::openai_live_transcription::Upsampler;
use crate::settings::get_settings;
use crate::vercel_transcription::{
    api_key, error_message, Request, PROTOCOL_VERSION, SPECIFICATION_VERSION,
};
use anyhow::{anyhow, Context, Result};
use futures_util::{Sink, SinkExt, StreamExt};
use log::{debug, info};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::{self, Message};

const STREAM_URL: &str = "wss://ai-gateway.vercel.sh/v4/ai/transcription-model";
/// Marks the handshake as a transcription stream; the key rides with it as
/// `ai-gateway-auth.<key>`, as the gateway takes it.
const SUBPROTOCOL: &str = "ai-gateway-transcription.v1";
/// The gateway rejects frames over 256 KiB; the envelope asks for at most
/// 64 KiB of audio per frame.
const MAX_AUDIO_FRAME_BYTES: usize = 64 * 1024;
/// How long the final transcript may take once the recording stops.
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);

/// One server part. Only the fields Handy reads.
#[derive(Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    delta: Option<String>,
    text: Option<String>,
    language: Option<String>,
    error: Option<Value>,
}

/// The audio a model is sent: OpenAI's realtime models take 24 kHz PCM16,
/// the others Handy's 16 kHz as is.
enum Audio {
    Native,
    Upsampled(Upsampler),
}

impl Audio {
    fn for_model(model: &str) -> Self {
        if model.starts_with("openai/") {
            Self::Upsampled(Upsampler::default())
        } else {
            Self::Native
        }
    }

    fn rate(&self) -> u32 {
        match self {
            Self::Native => 16_000,
            Self::Upsampled(_) => 24_000,
        }
    }

    fn convert(&mut self, pcm: &[f32]) -> Vec<i16> {
        match self {
            // The same conversion Handy saves WAV files with.
            Self::Native => pcm.iter().map(|s| (s * i16::MAX as f32) as i16).collect(),
            Self::Upsampled(upsampler) => upsampler.push(pcm),
        }
    }

    fn flush(&mut self) -> Vec<i16> {
        match self {
            Self::Native => Vec::new(),
            Self::Upsampled(upsampler) => upsampler.flush(),
        }
    }
}

/// The settled text so far and the segment still being heard: its deltas,
/// and the latest partial text.
#[derive(Default)]
struct Transcript {
    settled: String,
    deltas: String,
    partial: String,
}

impl Transcript {
    /// A delta supersedes the partial before it.
    fn push_delta(&mut self, delta: &str) {
        self.deltas.push_str(delta);
        self.partial.clear();
    }

    /// Providers mean different things by a partial: Gemini's is the whole
    /// segment so far, which its deltas then repeat; Microsoft MAI's is only
    /// what comes after the deltas.
    fn current(&self) -> String {
        if self.partial.trim_start().starts_with(self.deltas.trim()) {
            self.partial.clone()
        } else {
            format!("{}{}", self.deltas, self.partial)
        }
    }

    fn shown(&self) -> String {
        joined(&self.settled, self.current().trim())
    }

    fn settle(&mut self, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            self.settled = joined(&self.settled, text);
        }
        self.deltas.clear();
        self.partial.clear();
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
    let request = Request::from_settings(&settings, true)?;
    let mut audio = Audio::for_model(&request.model);

    let url = Url::parse_with_params(STREAM_URL, [("ai-model-id", &request.model)])?;
    let mut ws_request = url.as_str().into_client_request()?;
    let headers = ws_request.headers_mut();
    headers.insert(
        SEC_WEBSOCKET_PROTOCOL,
        format!("{SUBPROTOCOL}, ai-gateway-auth.{api_key}")
            .parse()
            .context("The AI Gateway API key has characters it can't be sent with")?,
    );
    headers.insert("ai-gateway-protocol-version", PROTOCOL_VERSION.parse()?);
    headers.insert(
        "ai-transcription-model-specification-version",
        SPECIFICATION_VERSION.parse()?,
    );
    headers.insert("ai-model-id", request.model.parse()?);
    headers.insert("ai-gateway-auth-method", "api-key".parse()?);

    let connect_started = Instant::now();
    let (socket, _) = tokio_tungstenite::connect_async(ws_request)
        .await
        .map_err(connect_error)?;
    debug!(
        "AI Gateway stream for {} connected in {:?}",
        request.model,
        connect_started.elapsed()
    );
    let (mut sink, mut events) = socket.split();

    let mut start = json!({
        "type": "transcription-stream.start",
        "inputAudioFormat": { "type": "audio/pcm", "rate": audio.rate() },
    });
    if let Some(options) = &request.provider_options {
        start["providerOptions"] = options.clone();
    }
    send(&mut sink, start).await?;

    let mut transcript = Transcript::default();
    let mut finished_at: Option<Instant> = None;
    loop {
        tokio::select! {
            command = commands.recv(), if reply.is_none() => match command {
                Some(Command::Audio(pcm)) => {
                    let pcm = audio.convert(&pcm);
                    send_audio(&mut sink, &pcm).await?;
                }
                Some(Command::Finish(result)) => {
                    let pcm = audio.flush();
                    send_audio(&mut sink, &pcm).await?;
                    // The server transcribes what it still holds, then sends
                    // `finish` and closes.
                    send(&mut sink, json!({ "type": "transcription-stream.audio-done" })).await?;
                    *reply = Some(result);
                    finished_at = Some(Instant::now());
                }
                None => return Ok(None),
            },
            event = events.next() => {
                let text = match event {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(frame))) => {
                        return Err(anyhow!("The AI Gateway closed the stream: {frame:?}"));
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e).context("The AI Gateway stream failed"),
                    None => return Err(anyhow!("The AI Gateway closed the stream")),
                };
                // Unknown or malformed parts are skipped, as the envelope asks.
                let Ok(part) = serde_json::from_str::<Part>(text.as_str()) else {
                    continue;
                };
                match part.kind.as_str() {
                    "transcript-delta" => {
                        transcript.push_delta(part.delta.as_deref().unwrap_or_default());
                        on_text(&transcript.shown());
                    }
                    "transcript-partial" => {
                        transcript.partial = part.text.unwrap_or_default();
                        on_text(&transcript.shown());
                    }
                    "transcript-final" => {
                        transcript.settle(part.text.as_deref().unwrap_or_default());
                        on_text(&transcript.settled);
                    }
                    "finish" => {
                        if let Some(finished_at) = finished_at {
                            info!(
                                "AI Gateway live transcription finished {:.2}s after the recording stopped",
                                finished_at.elapsed().as_secs_f64()
                            );
                        }
                        // The whole transcript; the parts so far if it has none.
                        let text = part.text.filter(|t| !t.trim().is_empty());
                        return Ok(Some(LiveText {
                            text: text.unwrap_or_else(|| transcript.shown()),
                            language: part.language,
                        }));
                    }
                    "error" => {
                        return Err(anyhow!(
                            "AI Gateway stream error: {}",
                            part_error(part.error)
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
                return Err(anyhow!("AI Gateway live transcription timed out"));
            }
        }
    }
}

/// An `error` part's message: `{ name, message }`, or whatever it carries.
fn part_error(error: Option<Value>) -> String {
    match error {
        Some(Value::String(message)) => message,
        Some(error) => error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| error.to_string()),
        None => "unknown error".to_string(),
    }
}

/// Why the stream didn't open: the gateway answers the upgrade with an HTTP
/// error (401 for a bad key, 404 for an unknown model) and a JSON body.
fn connect_error(error: tungstenite::Error) -> anyhow::Error {
    match error {
        tungstenite::Error::Http(response) => {
            let body = response
                .body()
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_default();
            anyhow!(
                "Couldn't connect to the AI Gateway (HTTP {}): {}",
                response.status().as_u16(),
                error_message(&body)
            )
        }
        e => anyhow!(e).context("Couldn't connect to the AI Gateway"),
    }
}

async fn send<S>(sink: &mut S, frame: Value) -> Result<()>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    sink.send(Message::text(frame.to_string()))
        .await
        .context("Couldn't send to the AI Gateway")
}

/// Sends PCM16 audio in binary frames of at most 64 KiB, if there is any.
async fn send_audio<S>(sink: &mut S, pcm: &[i16]) -> Result<()>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    for frame in bytes.chunks(MAX_AUDIO_FRAME_BYTES) {
        sink.send(Message::binary(frame.to_vec()))
            .await
            .context("Couldn't send audio to the AI Gateway")?;
    }
    Ok(())
}
