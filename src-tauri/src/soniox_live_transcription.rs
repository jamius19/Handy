/*
 * Author: Jamius Siam
 * Since: 13/11/2026
 */

//! Live transcription with the Soniox real-time model picked in settings
//! (e.g. `stt-rt-v6`): a WebSocket fed raw PCM while the recording goes. Each response carries the tokens
//! settled since the last one and the provisional tail, which drive the live
//! overlay; when the recording stops, an empty text frame makes Soniox settle
//! the rest and send `finished`.

use crate::cloud_live_transcription::{closed, connect_within, Command, LiveResult, LiveText};
use crate::deepgram_transcription::single_language;
use crate::settings::get_settings;
use crate::soniox_transcription::{
    api_key, stream_url, supports_strict, ErrorResponse, Request, Token,
};
use anyhow::{anyhow, Context, Result};
use futures_util::{Sink, SinkExt, StreamExt};
use log::{debug, info};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::mpsc;
use tokio::time::{interval_at, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{self, Message};

/// Handy records 16 kHz mono; Soniox takes it as is.
const SAMPLE_RATE: u32 = 16_000;
/// Soniox ends a stream that gets no audio or keepalive for 20 seconds.
const KEEP_ALIVE: Duration = Duration::from_secs(10);
/// How long the final transcript may take once the recording stops.
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);
/// Markers for an endpoint and a finalize request, not text.
const MARKERS: &[&str] = &["<end>", "<fin>"];

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    tokens: Vec<Token>,
    #[serde(default)]
    finished: bool,
    error_code: Option<u16>,
    #[serde(flatten)]
    error: ErrorResponse,
}

/// The settled text so far, and the languages it was heard in.
#[derive(Default)]
struct Transcript {
    text: String,
    languages: Vec<String>,
}

impl Transcript {
    /// Adds a response's settled tokens and returns the text with its
    /// provisional tail, which Soniox sends whole each time.
    fn apply(&mut self, tokens: Vec<Token>) -> String {
        let mut tail = String::new();
        for token in tokens {
            if MARKERS.contains(&token.text.as_str()) {
                continue;
            }
            if token.is_final {
                // Tokens carry their own spacing.
                self.text.push_str(&token.text);
                self.languages.extend(token.language);
            } else {
                tail.push_str(&token.text);
            }
        }
        format!("{}{tail}", self.text).trim().to_string()
    }

    fn into_live_text(self) -> LiveText {
        let language = single_language(self.languages.iter().map(String::as_str));
        LiveText {
            text: self.text.trim().to_string(),
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
    let model = settings.soniox_live_transcription_model.trim().to_string();
    if model.is_empty() {
        return Err(anyhow!(
            "No Soniox Live model is picked. Pick one in the model's settings."
        ));
    }
    let request = Request::from_settings(&settings);

    let region = settings.soniox_region;

    let connect_started = Instant::now();
    let mut ws_request = stream_url(region).into_client_request()?;
    ws_request
        .headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {api_key}").parse()?);
    // Checking the model against the catalogue, when it isn't known yet,
    // runs while the socket connects; a failed connect doesn't wait for it.
    let (strict_supported, (socket, _)) = tokio::try_join!(
        async {
            Ok::<_, anyhow::Error>(
                request.picked && supports_strict(region, &api_key, &model).await,
            )
        },
        async {
            connect_within("Soniox", tokio_tungstenite::connect_async(ws_request))
                .await?
                .context("Couldn't connect to Soniox")
        }
    )?;
    debug!(
        "Soniox {model} stream connected in {:?} (hints {:?})",
        connect_started.elapsed(),
        request.language_hints
    );
    let mut config = request.config(&model, strict_supported);
    config["audio_format"] = json!("pcm_s16le");
    config["sample_rate"] = json!(SAMPLE_RATE);
    config["num_channels"] = json!(1);
    let (mut sink, mut events) = socket.split();
    // A bad key still opens the socket; Soniox answers this with an error.
    send(&mut sink, Message::text(config.to_string())).await?;

    let mut transcript = Transcript::default();
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
                    // An empty text frame ends the audio; an empty binary
                    // one would be an empty chunk.
                    send(&mut sink, Message::text("")).await?;
                    *reply = Some(result);
                    finished_at = Some(Instant::now());
                }
                None => return Ok(None),
            },
            _ = keep_alive.tick(), if reply.is_none() => {
                send(&mut sink, Message::text(json!({ "type": "keepalive" }).to_string())).await?;
            }
            event = events.next() => {
                let text = match event {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(frame))) => {
                        return Err(anyhow!("Soniox closed the stream: {frame:?}"));
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(e).context("Soniox stream failed"),
                    None => return Err(anyhow!("Soniox closed the stream")),
                };
                let response: Response = serde_json::from_str(text.as_str())
                    .context("Invalid Soniox stream response")?;
                // Soniox sends the error, then closes the stream.
                if let Some(code) = response.error_code {
                    return Err(anyhow!(
                        "Soniox stream error (HTTP {code}): {}",
                        response.error.describe()
                    ));
                }
                if !response.tokens.is_empty() {
                    on_text(&transcript.apply(response.tokens));
                }
                if response.finished {
                    if let Some(finished_at) = finished_at {
                        info!(
                            "Soniox live transcription finished {:.2}s after the recording stopped",
                            finished_at.elapsed().as_secs_f64()
                        );
                    }
                    return Ok(Some(transcript.into_live_text()));
                }
            }
            // Nobody waits for the result any more (the dictation was cancelled).
            _ = closed(reply) => return Ok(None),
            _ = tokio::time::sleep_until(
                finished_at.unwrap_or_else(Instant::now) + FINISH_TIMEOUT
            ), if finished_at.is_some() => {
                return Err(anyhow!("Soniox live transcription timed out"));
            }
        }
    }
}

async fn send<S>(sink: &mut S, message: Message) -> Result<()>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    sink.send(message).await.context("Couldn't send to Soniox")
}

/// Sends 16 kHz mono audio as pcm_s16le, if there is any.
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
    send(sink, Message::binary(bytes)).await
}
