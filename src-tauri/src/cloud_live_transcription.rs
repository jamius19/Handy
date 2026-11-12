/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

//! The live session a streaming cloud model runs while the recording goes:
//! audio is sent as it is recorded and the final transcript comes back once
//! the recording stops. Each provider runs its own WebSocket session
//! (`openai_live_transcription.rs`, `deepgram_live_transcription.rs`,
//! `vercel_live_transcription.rs`, `soniox_live_transcription.rs`).

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::audio_toolkit::time_stretch::Stretcher;
use crate::deepgram_live_transcription;
use crate::managers::model::{speeds_up_audio, CloudProvider};
use crate::openai_live_transcription;
use crate::settings::get_settings;
use crate::soniox_live_transcription;
use crate::vercel_live_transcription;
use anyhow::{anyhow, Result};
use log::{debug, warn};
use std::future::Future;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::{mpsc, oneshot};

/// How long a live session's WebSocket may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// `connecting`, the opening of a live session's WebSocket to `provider`,
/// given up on after [`CONNECT_TIMEOUT`].
pub(crate) async fn connect_within<T>(
    provider: &str,
    connecting: impl Future<Output = T>,
) -> Result<T> {
    tokio::time::timeout(CONNECT_TIMEOUT, connecting)
        .await
        .map_err(|_| {
            anyhow!(
                "Couldn't connect to {provider} within {}s",
                CONNECT_TIMEOUT.as_secs()
            )
        })
}

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

/// One recording's live transcription. Dropping it ends the session unused
/// and discards Sonic's tail.
pub struct LiveTranscription {
    commands: mpsc::UnboundedSender<Command>,
    /// Speeds the audio up before it is sent; `None` at 1.0x.
    stretcher: Option<Stretcher>,
}

impl LiveTranscription {
    /// Connects to `provider` in the background to stream to `model_id`;
    /// audio fed before the session is up waits for it. `on_text` gets the
    /// transcript so far each time it changes.
    pub fn start(
        app: &AppHandle,
        provider: CloudProvider,
        model_id: &str,
        on_text: impl Fn(&str) + Send + 'static,
    ) -> Self {
        let (commands, rx) = mpsc::unbounded_channel();
        let stretcher = if speeds_up_audio(model_id) {
            Stretcher::new(get_settings(app).cloud_speed_percent, WHISPER_SAMPLE_RATE)
        } else {
            None
        };
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
                CloudProvider::Soniox => {
                    soniox_live_transcription::run(&app, rx, &mut reply, on_text).await
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
        Self {
            commands,
            stretcher,
        }
    }

    /// Sends 16 kHz mono audio, sped up when Speed is set.
    pub fn feed(&mut self, pcm: Vec<f32>) {
        let pcm = match &mut self.stretcher {
            Some(stretcher) => match stretcher.push(pcm) {
                Some(pcm) => pcm,
                None => return,
            },
            None => pcm,
        };
        let _ = self.commands.send(Command::Audio(pcm));
    }

    /// Ends the recording's audio. `reply` gets the final transcript, or the
    /// error that ended the session.
    pub fn finish(self, reply: LiveResult) {
        let Self {
            commands,
            stretcher,
        } = self;
        // The tail Sonic held back goes before the provider commits; not when
        // the session already failed.
        if let Some(stretcher) = stretcher.filter(|_| !commands.is_closed()) {
            if let Some(tail) = stretcher.finish() {
                let _ = commands.send(Command::Audio(tail));
            }
        }
        if let Err(mpsc::error::SendError(Command::Finish(reply))) =
            commands.send(Command::Finish(reply))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_toolkit::time_stretch::voice;

    fn session(percent: u8) -> (LiveTranscription, mpsc::UnboundedReceiver<Command>) {
        let (commands, rx) = mpsc::unbounded_channel();
        let stretcher = Stretcher::new(percent, 16_000);
        (
            LiveTranscription {
                commands,
                stretcher,
            },
            rx,
        )
    }

    fn received(rx: &mut mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn speed_100_sends_the_same_buffers() {
        let (mut live, mut rx) = session(100);
        let pcm = vec![0.1, 0.2, 0.3];
        let ptr = pcm.as_ptr();
        live.feed(pcm);
        match rx.try_recv() {
            Ok(Command::Audio(sent)) => {
                assert_eq!(sent.as_ptr(), ptr);
                assert_eq!(sent, vec![0.1, 0.2, 0.3]);
            }
            _ => panic!("expected audio"),
        }
    }

    #[test]
    fn no_audio_then_drop_sends_nothing() {
        let (live, mut rx) = session(200);
        drop(live);
        assert!(received(&mut rx).is_empty());
    }

    #[test]
    fn finish_sends_the_tail_before_finish() {
        let (mut live, mut rx) = session(200);
        for chunk in voice(16_000, 2) {
            live.feed(chunk);
        }
        let (reply, _result) = oneshot::channel();
        live.finish(reply);
        let commands = received(&mut rx);
        let n = commands.len();
        assert!(matches!(commands[n - 1], Command::Finish(_)));
        assert!(matches!(commands[n - 2], Command::Audio(_)));
        let mut total = 0;
        for command in &commands[..n - 1] {
            match command {
                Command::Audio(pcm) => {
                    assert!(!pcm.is_empty());
                    total += pcm.len();
                }
                Command::Finish(_) => panic!("Finish before the end"),
            }
        }
        assert!((total as f64 - 16_000.0).abs() <= 160.0, "{total} samples");
    }

    #[test]
    fn drop_after_audio_sends_no_finish() {
        let (mut live, mut rx) = session(200);
        for chunk in voice(16_000, 2) {
            live.feed(chunk);
        }
        drop(live);
        let mut total = 0;
        for command in received(&mut rx) {
            match command {
                Command::Audio(pcm) => total += pcm.len(),
                Command::Finish(_) => panic!("Finish after a drop"),
            }
        }
        assert!(total < 16_000);
    }

    #[test]
    fn finish_after_the_session_ended_skips_the_tail() {
        let (mut live, rx) = session(200);
        for chunk in voice(16_000, 1) {
            live.feed(chunk);
        }
        drop(rx);
        let (reply, mut result) = oneshot::channel();
        live.finish(reply);
        match result.try_recv() {
            Ok(Err(e)) => assert!(e.to_string().contains("already ended")),
            _ => panic!("expected the already-ended error"),
        }
    }
}
