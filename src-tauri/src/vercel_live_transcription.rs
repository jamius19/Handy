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

use crate::cloud_live_transcription::{closed, connect_within, Command, LiveResult, LiveText};
use crate::deepgram_live_transcription::joined;
use crate::openai_live_transcription::Upsampler;
use crate::settings::get_settings;
use crate::vercel_transcription::{
    api_key, error_message, Request, PROTOCOL_VERSION, SPECIFICATION_VERSION,
};
use anyhow::{anyhow, Context, Result};
use futures_util::{Sink, SinkExt, StreamExt};
use log::{debug, info, warn};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::AppHandle;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const STREAM_URL: &str = "wss://ai-gateway.vercel.sh/v4/ai/transcription-model";
/// Marks the handshake as a transcription stream; the key rides with it as
/// `ai-gateway-auth.<key>`, as the gateway takes it.
const SUBPROTOCOL: &str = "ai-gateway-transcription.v1";
/// The gateway rejects frames over 256 KiB; the envelope asks for at most
/// 64 KiB of audio per frame.
const MAX_AUDIO_FRAME_BYTES: usize = 64 * 1024;
/// How long the final transcript may take once the recording stops.
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);
/// How long reconnecting a failed stream and sending its audio again may take.
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// One server part. Only the fields Handy reads.
#[derive(Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    delta: Option<String>,
    text: Option<String>,
    language: Option<String>,
    error: Option<Value>,
    #[serde(rename = "startSecond")]
    start_second: Option<f64>,
    #[serde(rename = "durationInSeconds")]
    duration_in_seconds: Option<f64>,
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
/// its earlier partial chunks, and the latest partial text.
#[derive(Default)]
struct Transcript {
    settled: String,
    deltas: String,
    /// Partials that a later one followed on from rather than revised.
    chunks: String,
    partial: String,
    /// Where the latest partial starts and ends in the audio, when the part
    /// says.
    partial_span: Option<(f64, f64)>,
}

/// How far apart two reported times may be and still count as the same,
/// for rounding in them.
const CHUNK_TOLERANCE_SECONDS: f64 = 0.05;

impl Transcript {
    /// A partial revises the one before it, unless it starts later, where
    /// that one ended: Grok STT sends each ~3s chunk of an utterance as its
    /// own partial, so the earlier chunks are kept until the utterance's
    /// final. A revision starts where the one it revises did (or earlier).
    fn push_partial(&mut self, text: String, start: Option<f64>, duration: Option<f64>) {
        if let (Some(next), Some((start, end))) = (start, self.partial_span) {
            if next > start + CHUNK_TOLERANCE_SECONDS && next >= end - CHUNK_TOLERANCE_SECONDS {
                self.chunks = joined(&self.chunks, self.partial.trim());
            }
        }
        self.partial = text;
        self.partial_span = start
            .zip(duration)
            .map(|(start, duration)| (start, start + duration));
    }

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
        let segment = joined(&self.chunks, self.current().trim());
        joined(&self.settled, segment.trim())
    }

    fn settle(&mut self, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            self.settled = joined(&self.settled, text);
        }
        self.deltas.clear();
        self.chunks.clear();
        self.partial.clear();
        self.partial_span = None;
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
    let rate = audio.rate();

    // The first connect is bounded like any provider's; a reconnect has its own
    // bound (`reconnect`).
    let (mut sink, mut events) =
        connect_within("the AI Gateway", connect(&api_key, &request, rate))
            .await??
            .split();

    let mut transcript = Transcript::default();
    // The text last sent to the overlay.
    let mut shown = String::new();
    let mut finished_at: Option<Instant> = None;
    // When the final transcript is overdue, once the recording has stopped.
    let mut deadline: Option<Instant> = None;
    // Every sample sent, to send again to a new stream if this one fails;
    // `None` once that one retry is used.
    let mut sent: Option<Vec<i16>> = Some(Vec::new());
    loop {
        // Why the stream failed, when it did.
        let failure = tokio::select! {
            command = commands.recv(), if reply.is_none() => match command {
                Some(Command::Audio(pcm)) => {
                    let pcm = audio.convert(&pcm);
                    if let Some(sent) = &mut sent {
                        sent.extend_from_slice(&pcm);
                    }
                    match send_audio(&mut sink, &pcm).await {
                        Ok(()) => continue,
                        Err(failure) => failure,
                    }
                }
                Some(Command::Finish(result)) => {
                    let pcm = audio.flush();
                    if let Some(sent) = &mut sent {
                        sent.extend_from_slice(&pcm);
                    }
                    *reply = Some(result);
                    finished_at = Some(Instant::now());
                    deadline = Some(Instant::now() + FINISH_TIMEOUT);
                    // The server transcribes what it still holds, then sends
                    // `finish` and closes.
                    let done = match send_audio(&mut sink, &pcm).await {
                        Ok(()) => {
                            send(&mut sink, json!({ "type": "transcription-stream.audio-done" }))
                                .await
                        }
                        Err(failure) => Err(failure),
                    };
                    match done {
                        Ok(()) => continue,
                        Err(failure) => failure,
                    }
                }
                None => return Ok(None),
            },
            event = events.next() => {
                let received = match event {
                    Some(Ok(Message::Text(text))) => {
                        // Unknown or malformed parts are skipped, as the envelope asks.
                        let Ok(part) = serde_json::from_str::<Part>(text.as_str()) else {
                            continue;
                        };
                        if part.kind == "error" {
                            Err(anyhow!("AI Gateway stream error: {}", part_error(part.error)))
                        } else {
                            Ok(part)
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        Err(anyhow!("The AI Gateway closed the stream: {frame:?}"))
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => Err(anyhow!(e).context("The AI Gateway stream failed")),
                    None => Err(anyhow!("The AI Gateway closed the stream")),
                };
                match received {
                    Err(failure) => failure,
                    Ok(part) => {
                        match part.kind.as_str() {
                            "transcript-delta" => {
                                transcript.push_delta(part.delta.as_deref().unwrap_or_default());
                                let next = transcript.shown();
                                if !only_trims(&shown, &next) {
                                    shown = next;
                                    on_text(&shown);
                                }
                            }
                            "transcript-partial" => {
                                transcript.push_partial(
                                    part.text.unwrap_or_default(),
                                    part.start_second,
                                    part.duration_in_seconds,
                                );
                                let next = transcript.shown();
                                if !only_trims(&shown, &next) {
                                    shown = next;
                                    on_text(&shown);
                                }
                            }
                            "transcript-final" => {
                                transcript.settle(part.text.as_deref().unwrap_or_default());
                                shown = transcript.settled.clone();
                                on_text(&shown);
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
                            _ => {}
                        }
                        continue;
                    }
                }
            }
            // Nobody waits for the result any more (the dictation was cancelled).
            _ = closed(reply) => return Ok(None),
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)),
                if deadline.is_some() =>
            {
                return Err(anyhow!("AI Gateway live transcription timed out"));
            }
        };

        // The provider sometimes fails a stream for no reason it gives; a new
        // one usually works, so retry once before giving up.
        let Some(resend) = sent.take() else {
            return Err(failure);
        };
        warn!(
            "AI Gateway live stream for {} failed ({failure:#}); reconnecting once and \
             resending {:.1}s of audio",
            request.model,
            resend.len() as f64 / rate as f64
        );
        let stopped = finished_at.is_some();
        (sink, events) = reconnect(&api_key, &request, rate, &resend, stopped)
            .await?
            .split();
        // The new stream transcribes the recording from its start, and gets
        // its own time for the final transcript.
        transcript = Transcript::default();
        if stopped {
            deadline = Some(Instant::now() + FINISH_TIMEOUT);
        }
        info!("AI Gateway live stream for {} reconnected", request.model);
    }
}

/// A new stream, sent `resend` and, once the recording has stopped,
/// `audio-done`; bounded so a stuck reconnect can't hold up the dictation.
async fn reconnect(
    api_key: &str,
    request: &Request,
    rate: u32,
    resend: &[i16],
    stopped: bool,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
    let reconnecting = async {
        let mut socket = connect(api_key, request, rate).await?;
        send_audio(&mut socket, resend).await?;
        if stopped {
            send(
                &mut socket,
                json!({ "type": "transcription-stream.audio-done" }),
            )
            .await?;
        }
        Ok(socket)
    };
    tokio::time::timeout(RECONNECT_TIMEOUT, reconnecting)
        .await
        .map_err(|_| anyhow!("Reconnecting to the AI Gateway timed out"))?
}

/// Opens a stream to the gateway for `request`'s model and starts it.
async fn connect(
    api_key: &str,
    request: &Request,
    rate: u32,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
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
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_request)
        .await
        .map_err(connect_error)?;
    debug!(
        "AI Gateway stream for {} connected in {:?}",
        request.model,
        connect_started.elapsed()
    );

    let mut start = json!({
        "type": "transcription-stream.start",
        "inputAudioFormat": { "type": "audio/pcm", "rate": rate },
    });
    if let Some(options) = &request.provider_options {
        start["providerOptions"] = options.clone();
    }
    send(&mut socket, start).await?;
    Ok(socket)
}

/// Whether `next` only cuts words off the end of `shown`. Such text isn't
/// shown while the stream goes, as the cut words come straight back: Grok STT
/// ends a chunk short of the guess before it and sends the rest with the next
/// chunk, and Microsoft MAI sends settled words as a delta just before the
/// partial with the rest. The final is always shown.
fn only_trims(shown: &str, next: &str) -> bool {
    next.len() < shown.len() && shown.starts_with(next)
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
