/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

//! Sends a cloud model's transcription to the API it runs on.

use crate::cloud_live_transcription::LiveText;
use crate::deepgram_transcription;
use crate::managers::model::{cloud_provider, CloudProvider};
use crate::managers::transcription::Transcription;
use crate::openai_transcription;
use anyhow::{anyhow, Result};
use log::{info, warn};
use tauri::AppHandle;
use tokio::sync::oneshot;

/// Transcribe 16 kHz mono `samples` in one request to `model_id`'s API.
pub async fn transcribe(app: &AppHandle, model_id: &str, samples: &[f32]) -> Result<Transcription> {
    match cloud_provider(model_id) {
        Some(CloudProvider::OpenAi) => openai_transcription::transcribe(app, samples).await,
        Some(CloudProvider::Deepgram) => deepgram_transcription::transcribe(app, samples).await,
        None => Err(anyhow!("'{model_id}' is not a cloud model")),
    }
}

/// A dictation's transcription with a cloud model: `live`'s text when the
/// model streamed the recording, else `samples` sent in one request.
pub async fn transcribe_recording(
    app: &AppHandle,
    model_id: &str,
    samples: &[f32],
    live: Option<oneshot::Receiver<Result<LiveText>>>,
) -> Result<Transcription> {
    if let Some(live) = live {
        match live.await {
            // A stream can miss what a run over the whole recording finds,
            // so empty text retries in one request, as for a local stream.
            Ok(Ok(live)) if !live.text.trim().is_empty() => {
                return Ok(match cloud_provider(model_id) {
                    Some(CloudProvider::Deepgram) => deepgram_transcription::finish_live(app, live),
                    _ => openai_transcription::finish_live(app, live),
                });
            }
            Ok(Ok(_)) => info!("Live transcription produced no text; retrying in one request"),
            Ok(Err(e)) => warn!("Live transcription failed ({e:#}); retrying in one request"),
            Err(_) => info!("No live transcription result; transcribing in one request"),
        }
    }
    transcribe(app, model_id, samples).await
}
