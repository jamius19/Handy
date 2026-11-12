/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

//! Sends a cloud model's transcription to the API it runs on.

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
use crate::audio_toolkit::time_stretch::Stretcher;
use crate::cloud_live_transcription::LiveText;
use crate::deepgram_transcription;
use crate::managers::model::{cloud_provider, speeds_up_audio, CloudProvider};
use crate::managers::transcription::Transcription;
use crate::openai_transcription;
use crate::openrouter_transcription;
use crate::settings::get_settings;
use crate::soniox_transcription;
use crate::vercel_transcription;
use anyhow::{anyhow, Result};
use log::{info, warn};
use tauri::AppHandle;
use tokio::sync::oneshot;

/// The API `model_id` runs on, or an error for a local model.
fn provider_of(model_id: &str) -> Result<CloudProvider> {
    cloud_provider(model_id).ok_or_else(|| anyhow!("'{model_id}' is not a cloud model"))
}

/// Transcribe 16 kHz mono `samples` in one request to `model_id`'s API.
pub async fn transcribe(app: &AppHandle, model_id: &str, samples: &[f32]) -> Result<Transcription> {
    let provider = provider_of(model_id)?;
    let percent = if speeds_up_audio(model_id) {
        get_settings(app).cloud_speed_percent
    } else {
        100
    };
    let stretched = stretch_for_upload(percent, samples);
    let samples = stretched.as_deref().unwrap_or(samples);
    match provider {
        CloudProvider::OpenAi => openai_transcription::transcribe(app, samples).await,
        CloudProvider::Deepgram => deepgram_transcription::transcribe(app, samples).await,
        CloudProvider::OpenRouter => openrouter_transcription::transcribe(app, samples).await,
        CloudProvider::VercelGateway => vercel_transcription::transcribe(app, samples).await,
        CloudProvider::Soniox => soniox_transcription::transcribe(app, samples).await,
    }
}

/// `samples` sped up for upload, padded again if that left them under the
/// second a short recording is padded from (`AudioRecordingManager::stop_recording`);
/// `None` to send them as they are (1.0x, or no audio).
fn stretch_for_upload(percent: u8, samples: &[f32]) -> Option<Vec<f32>> {
    if samples.is_empty() {
        return None;
    }
    let mut stretcher = Stretcher::new(percent, WHISPER_SAMPLE_RATE)?;
    let rate = WHISPER_SAMPLE_RATE as usize;
    let mut out = Vec::new();
    for piece in samples.chunks(rate) {
        out.extend(stretcher.push(piece.to_vec()).unwrap_or_default());
    }
    out.extend(stretcher.finish().unwrap_or_default());
    if !out.is_empty() && out.len() < rate {
        out.resize(rate * 5 / 4, 0.0);
    }
    Some(out)
}

/// A dictation's transcription with a cloud model: `live`'s text when the
/// model streamed the recording, else `samples` sent in one request.
pub async fn transcribe_recording(
    app: &AppHandle,
    model_id: &str,
    samples: &[f32],
    live: Option<oneshot::Receiver<Result<LiveText>>>,
) -> Result<Transcription> {
    let provider = provider_of(model_id)?;
    if let Some(live) = live {
        match live.await {
            // A stream can miss what a run over the whole recording finds,
            // so empty text retries in one request, as for a local stream.
            Ok(Ok(live)) if !live.text.trim().is_empty() => {
                return Ok(match provider {
                    CloudProvider::OpenAi => openai_transcription::finish_live(app, live),
                    CloudProvider::Deepgram => deepgram_transcription::finish_live(app, live),
                    CloudProvider::OpenRouter => openrouter_transcription::finish_live(app, live),
                    CloudProvider::VercelGateway => vercel_transcription::finish_live(app, live),
                    CloudProvider::Soniox => soniox_transcription::finish_live(app, live),
                });
            }
            Ok(Ok(_)) => info!("Live transcription produced no text; retrying in one request"),
            Ok(Err(e)) => warn!("Live transcription failed ({e:#}); retrying in one request"),
            Err(_) => info!("No live transcription result; transcribing in one request"),
        }
    }
    transcribe(app, model_id, samples).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_toolkit::time_stretch::voice;

    fn voice_samples(seconds: u32) -> Vec<f32> {
        voice(16_000, seconds).concat()
    }

    #[test]
    fn nothing_to_stretch() {
        assert!(stretch_for_upload(100, &voice_samples(1)).is_none());
        assert!(stretch_for_upload(200, &[]).is_none());
    }

    #[test]
    fn halves_the_audio_at_200() {
        let out = stretch_for_upload(200, &voice_samples(5)).unwrap();
        assert!(
            (out.len() as f64 - 40_000.0).abs() <= 400.0,
            "{}",
            out.len()
        );
    }

    #[test]
    fn short_result_is_padded_again() {
        let samples = &voice_samples(2)[..30_400];
        assert_eq!(stretch_for_upload(200, samples).unwrap().len(), 20_000);
    }

    #[test]
    fn padded_recording_stays_padded() {
        let mut samples = voice_samples(1)[..8_000].to_vec();
        samples.resize(20_000, 0.0);
        assert_eq!(stretch_for_upload(200, &samples).unwrap().len(), 20_000);
    }
}
