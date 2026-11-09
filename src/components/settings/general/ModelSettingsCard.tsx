import React from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { LanguageSelector } from "../LanguageSelector";
import { TranslateToEnglish } from "../TranslateToEnglish";
import { CloudApiKey } from "../CloudApiKey";
import { DeepgramApiKey } from "../DeepgramApiKey";
import { LiveTranscribeDelay } from "../LiveTranscribeDelay";
import {
  cloudProviderOf,
  LIVE_CLOUD_MODEL_ID,
} from "../../shared/modelTable/tableModel";
import { useModelStore } from "../../../stores/modelStore";
import type { CloudProvider, ModelInfo } from "@/bindings";

/** Each provider's API key field. A new provider won't compile until added. */
const API_KEY_FIELDS: Record<
  CloudProvider,
  React.FC<{ descriptionMode?: "inline" | "tooltip"; grouped?: boolean }>
> = {
  OpenAi: CloudApiKey,
  Deepgram: DeepgramApiKey,
};

export const ModelSettingsCard: React.FC = () => {
  const { t } = useTranslation();
  const { currentModel, models } = useModelStore();

  const currentModelInfo = models.find((m: ModelInfo) => m.id === currentModel);

  const showLanguageSelector =
    currentModelInfo?.supports_language_selection ?? false;
  const supportsTranslation = currentModelInfo?.supports_translation ?? false;
  const provider = currentModelInfo ? cloudProviderOf(currentModelInfo) : null;
  const ApiKeyField = provider ? API_KEY_FIELDS[provider] : null;
  // Only GPT-Live-Transcribe has a delay.
  const isLiveCloud = currentModelInfo?.id === LIVE_CLOUD_MODEL_ID;
  const hasAnySettings =
    showLanguageSelector || supportsTranslation || !!ApiKeyField;

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
      {ApiKeyField && <ApiKeyField descriptionMode="tooltip" grouped={true} />}
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
