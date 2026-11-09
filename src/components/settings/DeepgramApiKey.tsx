/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

import React from "react";
import { useTranslation } from "react-i18next";
import { SettingContainer } from "../ui/SettingContainer";
import { ApiKeyField } from "./PostProcessingSettingsApi/ApiKeyField";
import { useSettings } from "../../hooks/useSettings";

interface DeepgramApiKeyProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/** The Deepgram API key the Nova-3 cloud model transcribes with. */
export const DeepgramApiKey: React.FC<DeepgramApiKeyProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    return (
      <SettingContainer
        title={t("settings.cloudModel.deepgramApiKey.title")}
        description={t("settings.cloudModel.deepgramApiKey.description")}
        descriptionMode={descriptionMode}
        layout="horizontal"
        grouped={grouped}
      >
        <ApiKeyField
          value={getSetting("deepgram_api_key") ?? ""}
          onBlur={(value) => updateSetting("deepgram_api_key", value.trim())}
          placeholder={t("settings.cloudModel.deepgramApiKey.placeholder")}
          disabled={isUpdating("deepgram_api_key")}
          className="min-w-[320px]"
        />
      </SettingContainer>
    );
  },
);
