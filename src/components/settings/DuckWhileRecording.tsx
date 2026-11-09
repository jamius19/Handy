import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface DuckWhileRecordingToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const DuckWhileRecording: React.FC<DuckWhileRecordingToggleProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const duckEnabled = getSetting("duck_while_recording") ?? false;
    const muteEnabled = getSetting("mute_while_recording") ?? false;

    // Mute and duck are exclusive: turning one on turns the other off.
    const handleChange = async (enabled: boolean) => {
      if (enabled && muteEnabled) {
        await updateSetting("mute_while_recording", false);
      }
      await updateSetting("duck_while_recording", enabled);
    };

    return (
      <ToggleSwitch
        checked={duckEnabled}
        onChange={handleChange}
        isUpdating={isUpdating("duck_while_recording")}
        label={t("settings.debug.duckWhileRecording.label")}
        description={t("settings.debug.duckWhileRecording.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  });
