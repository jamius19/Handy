import React from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { LanguageSelector } from "../LanguageSelector";
import { TranslateToEnglish } from "../TranslateToEnglish";
import { CloudApiKey } from "../CloudApiKey";
import { DeepgramApiKey } from "../DeepgramApiKey";
import { LiveTranscribeDelay } from "../LiveTranscribeDelay";
import {
  DEEPGRAM_MODEL_ID,
  isCloudModel,
  LIVE_CLOUD_MODEL_ID,
} from "../../shared/modelTable/tableModel";
import { useModelStore } from "../../../stores/modelStore";
import type { ModelInfo } from "@/bindings";

export const ModelSettingsCard: React.FC = () => {
  const { t } = useTranslation();
  const { currentModel, models } = useModelStore();

  const currentModelInfo = models.find((m: ModelInfo) => m.id === currentModel);

  const showLanguageSelector =
    currentModelInfo?.supports_language_selection ?? false;
  const supportsTranslation = currentModelInfo?.supports_translation ?? false;
  const isCloud = !!currentModelInfo && isCloudModel(currentModelInfo);
  // Only GPT-Live-Transcribe has a delay.
  const isLiveCloud = currentModelInfo?.id === LIVE_CLOUD_MODEL_ID;
  // Deepgram has its own key; the OpenAI models share post-processing's.
  const isDeepgram = currentModelInfo?.id === DEEPGRAM_MODEL_ID;
  const hasAnySettings = showLanguageSelector || supportsTranslation || isCloud;

  // Don't render anything if no model is selected or no settings available
  if (!currentModel || !currentModelInfo || !hasAnySettings) {
    return null;
  }

  return (
    <SettingsGroup
      title={t("settings.modelSettings.title", {
        model: currentModelInfo.name,
      })}
    >
      {isCloud &&
        (isDeepgram ? (
          <DeepgramApiKey descriptionMode="tooltip" grouped={true} />
        ) : (
          <CloudApiKey descriptionMode="tooltip" grouped={true} />
        ))}
      {isLiveCloud && (
        <LiveTranscribeDelay descriptionMode="tooltip" grouped={true} />
      )}
      {showLanguageSelector && (
        <LanguageSelector
          descriptionMode="tooltip"
          grouped={true}
          modelId={currentModelInfo.id}
          supportedLanguages={currentModelInfo.supported_languages}
        />
      )}
      {supportsTranslation && (
        <TranslateToEnglish descriptionMode="tooltip" grouped={true} />
      )}
    </SettingsGroup>
  );
};
