/*
 * Author: Jamius Siam
 * Since: 05/11/2026
 */

import React from "react";
import { useTranslation } from "react-i18next";
import { SettingContainer } from "../ui/SettingContainer";
import { ApiKeyField } from "./PostProcessingSettingsApi/ApiKeyField";
import { useSettings } from "../../hooks/useSettings";

/** The provider whose key the cloud model uses, shared with post-processing. */
export const CLOUD_API_KEY_PROVIDER = "openai";

interface CloudApiKeyProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/** The OpenAI API key the cloud model transcribes with. */
export const CloudApiKey: React.FC<CloudApiKeyProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { settings, updatePostProcessApiKey, isUpdating } = useSettings();
    const apiKey =
      settings?.post_process_api_keys?.[CLOUD_API_KEY_PROVIDER] ?? "";

    return (
      <SettingContainer
        title={t("settings.cloudModel.apiKey.title")}
        description={t("settings.cloudModel.apiKey.description")}
        descriptionMode={descriptionMode}
        layout="horizontal"
        grouped={grouped}
      >
        <ApiKeyField
          value={apiKey}
          onBlur={(value) =>
            updatePostProcessApiKey(CLOUD_API_KEY_PROVIDER, value.trim())
          }
          placeholder={t("settings.cloudModel.apiKey.placeholder")}
          disabled={isUpdating(
            `post_process_api_key:${CLOUD_API_KEY_PROVIDER}`,
          )}
          className="min-w-[320px]"
        />
      </SettingContainer>
    );
  },
);
