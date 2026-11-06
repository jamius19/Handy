/*
 * Author: Jamius Siam
 * Since: 06/11/2026
 */

import React from "react";
import { useTranslation } from "react-i18next";
import type { LiveTranscribeDelay as Delay } from "@/bindings";
import { Dropdown, SettingContainer } from "@/components/ui";
import { useSettings } from "../../hooks/useSettings";

const DELAYS: Delay[] = ["minimal", "low", "medium", "high", "xhigh"];

interface LiveTranscribeDelayProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/** How long GPT-Live-Transcribe waits before it writes text. */
export const LiveTranscribeDelay: React.FC<LiveTranscribeDelayProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    return (
      <SettingContainer
        title={t("settings.cloudModel.liveDelay.title")}
        description={t("settings.cloudModel.liveDelay.description")}
        descriptionMode={descriptionMode}
        layout="horizontal"
        grouped={grouped}
      >
        <Dropdown
          options={DELAYS.map((delay) => ({
            value: delay,
            label: t(`settings.cloudModel.liveDelay.delays.${delay}`),
          }))}
          selectedValue={getSetting("openai_live_transcribe_delay") ?? "medium"}
          onSelect={(value) =>
            updateSetting("openai_live_transcribe_delay", value as Delay)
          }
          disabled={isUpdating("openai_live_transcribe_delay")}
        />
      </SettingContainer>
    );
  });
