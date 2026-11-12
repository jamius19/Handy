import React from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { LanguageSelector } from "../LanguageSelector";
import { TranslateToEnglish } from "../TranslateToEnglish";
import { CloudApiKey } from "../CloudApiKey";
import { DeepgramApiKey } from "../DeepgramApiKey";
import { SonioxSettings } from "../SonioxSettings";
import { OpenRouterSettings } from "../OpenRouterSettings";
import { VercelGatewaySettings } from "../VercelGatewaySettings";
import { LiveTranscribeDelay } from "../LiveTranscribeDelay";
import { CloudSpeed } from "../CloudSpeed";
import {
  cloudProviderOf,
  LIVE_CLOUD_MODEL_ID,
  SONIOX_LIVE_CLOUD_MODEL_ID,
} from "../../shared/modelTable/tableModel";
import { useModelStore } from "../../../stores/modelStore";
import type { CloudProvider, ModelInfo } from "@/bindings";

/** Each provider's settings fields (its API key, and its model picker where
 * it has one). A new provider won't compile until added. `modelId` tells a
 * provider with several models which one is selected. */
const API_KEY_FIELDS: Record<
  CloudProvider,
  React.FC<{
    descriptionMode?: "inline" | "tooltip";
    grouped?: boolean;
    modelId?: string;
  }>
> = {
  OpenAi: CloudApiKey,
  Deepgram: DeepgramApiKey,
  OpenRouter: OpenRouterSettings,
  VercelGateway: VercelGatewaySettings,
  Soniox: SonioxSettings,
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
  // Soniox bills a live stream for how long it's open, so Speed saves
  // nothing there and isn't applied (`speeds_up_audio` in Rust).
  const hasSpeed =
    !!provider && currentModelInfo?.id !== SONIOX_LIVE_CLOUD_MODEL_ID;
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
      {ApiKeyField && (
        <ApiKeyField
          descriptionMode="tooltip"
          grouped={true}
          modelId={currentModelInfo.id}
        />
      )}
      {isLiveCloud && (
        <LiveTranscribeDelay descriptionMode="tooltip" grouped={true} />
      )}
      {hasSpeed && <CloudSpeed descriptionMode="tooltip" grouped={true} />}
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
