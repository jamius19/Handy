/*
 * Author: Jamius Siam
 * Since: 04/11/2026
 */

import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface OpenAiTranscriptionProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const OpenAiTranscription: React.FC<OpenAiTranscriptionProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("openai_transcription_enabled") || false;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(value) =>
          updateSetting("openai_transcription_enabled", value)
        }
        isUpdating={isUpdating("openai_transcription_enabled")}
        label={t("settings.openaiTranscription.label")}
        description={t("settings.openaiTranscription.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  });
