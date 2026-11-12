/*
 * Author: Jamius Siam
 * Since: 12/11/2026
 */

import React from "react";
import { useTranslation } from "react-i18next";
import { Slider } from "../ui/Slider";
import { useSettings } from "../../hooks/useSettings";

interface CloudSpeedProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/** How much faster a cloud model is sent the audio. */
export const CloudSpeed: React.FC<CloudSpeedProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting } = useSettings();

    return (
      <Slider
        value={getSetting("cloud_speed_percent") ?? 100}
        onChange={(value) => updateSetting("cloud_speed_percent", value)}
        min={100}
        max={200}
        step={10}
        label={t("settings.cloudModel.speed.title")}
        description={t("settings.cloudModel.speed.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        formatValue={(value) => `${(value / 100).toFixed(1)}x`}
      />
    );
  },
);
