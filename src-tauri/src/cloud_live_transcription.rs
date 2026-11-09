/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

//! The live session a streaming cloud model runs while the recording goes:
//! audio is sent as it is recorded and the final transcript comes back once
//! the recording stops. Each provider runs its own WebSocket session
//! (`openai_live_transcription.rs`, `deepgram_live_transcription.rs`,
//! `vercel_live_transcription.rs`).

use crate::deepgram_live_transcription;
use crate::managers::model::CloudProvider;
use crate::openai_live_transcription;
use crate::vercel_live_transcription;
use anyhow::{anyhow, Result};
use log::{debug, warn};
use tauri::AppHandle;
use tokio::sync::{mpsc, oneshot};

/// A live session's final transcript.
pub struct LiveText {
    pub text: String,
    /// The one language the model heard, when it reports one.
    pub language: Option<String>,
}

/// The final transcript, or why there is none.
pub type LiveResult = oneshot::Sender<Result<LiveText>>;

pub(crate) enum Command {
    Audio(Vec<f32>),
    Finish(LiveResult),
}

/// One recording's live transcription. Dropping it ends the session unused.
pub struct LiveTranscription {
    commands: mpsc::UnboundedSender<Command>,
}

impl LiveTranscription {
    /// Connects to `provider` in the background; audio fed before the
    /// session is up waits for it. `on_text` gets the transcript so far each
    /// time it changes.
    pub fn start(
        app: &AppHandle,
        provider: CloudProvider,
        on_text: impl Fn(&str) + Send + 'static,
    ) -> Self {
        let (commands, rx) = mpsc::unbounded_channel();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut reply = None;
            let result = match provider {
                CloudProvider::OpenAi => {
                    openai_live_transcription::run(&app, rx, &mut reply, on_text).await
                }
                CloudProvider::Deepgram => {
                    deepgram_live_transcription::run(&app, rx, &mut reply, on_text).await
                }
                // Its row says it doesn't stream, so no session starts for it.
                CloudProvider::OpenRouter => {
                    Err(anyhow!("OpenRouter models don't transcribe live"))
                }
                CloudProvider::VercelGateway => {
                    vercel_live_transcription::run(&app, rx, &mut reply, on_text).await
                }
            };
            match (reply, result) {
                (Some(reply), result) => {
                    let _ = reply.send(result.and_then(|text| {
                        text.ok_or_else(|| anyhow!("The live transcription was dropped"))
                    }));
                }
                (None, Err(e)) => warn!("Live transcription ended early: {e:#}"),
                (None, Ok(_)) => debug!("Live transcription cancelled"),
            }
        });
        Self { commands }
    }

    /// Sends 16 kHz mono audio.
    pub fn feed(&self, pcm: Vec<f32>) {
        let _ = self.commands.send(Command::Audio(pcm));
    }

    /// Ends the recording's audio. `reply` gets the final transcript, or the
    /// error that ended the session.
    pub fn finish(self, reply: LiveResult) {
        if let Err(mpsc::error::SendError(Command::Finish(reply))) =
            self.commands.send(Command::Finish(reply))
        {
            let _ = reply.send(Err(anyhow!(
                "The live transcription session had already ended"
            )));
        }
    }
}

/// Resolves once `reply`'s receiver is dropped; never while there is none.
pub(crate) async fn closed(reply: &mut Option<LiveResult>) {
    match reply {
        Some(reply) => reply.closed().await,
        None => std::future::pending().await,
    }
}
