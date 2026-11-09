/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

import React from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { SettingContainer } from "../ui/SettingContainer";
import { ApiKeyField } from "./PostProcessingSettingsApi/ApiKeyField";
import { CloudModelPicker } from "./CloudModelPicker";
import { useSettings } from "../../hooks/useSettings";

/** The provider whose key the OpenRouter model uses, shared with post-processing. */
const OPENROUTER_PROVIDER = "openrouter";

interface OpenRouterSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/** The OpenRouter API key and the OpenRouter model that transcribes. */
export const OpenRouterSettings: React.FC<OpenRouterSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { settings, updatePostProcessApiKey, isUpdating } = useSettings();
    const apiKey = settings?.post_process_api_keys?.[OPENROUTER_PROVIDER] ?? "";

    return (
      <>
        <SettingContainer
          title={t("settings.cloudModel.openRouterApiKey.title")}
          description={t("settings.cloudModel.openRouterApiKey.description")}
          descriptionMode={descriptionMode}
          layout="horizontal"
          grouped={grouped}
        >
          <ApiKeyField
            value={apiKey}
            onBlur={(value) =>
              updatePostProcessApiKey(OPENROUTER_PROVIDER, value.trim())
            }
            placeholder={t("settings.cloudModel.openRouterApiKey.placeholder")}
            disabled={isUpdating(`post_process_api_key:${OPENROUTER_PROVIDER}`)}
            className="min-w-[320px]"
          />
        </SettingContainer>
        <CloudModelPicker
          setting="openrouter_transcription_model"
          textKey="settings.cloudModel.openRouterModel"
          fetchModels={commands.fetchOpenrouterTranscriptionModels}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
      </>
    );
  },
);
