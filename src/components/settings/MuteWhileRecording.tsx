import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface MuteWhileRecordingToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const MuteWhileRecording: React.FC<MuteWhileRecordingToggleProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const muteEnabled = getSetting("mute_while_recording") ?? false;
    const duckEnabled = getSetting("duck_while_recording") ?? false;

    // Mute and duck are exclusive: turning one on turns the other off.
    const handleChange = async (enabled: boolean) => {
      if (enabled && duckEnabled) {
        await updateSetting("duck_while_recording", false);
      }
      await updateSetting("mute_while_recording", enabled);
    };

    return (
      <ToggleSwitch
        checked={muteEnabled}
        onChange={handleChange}
        isUpdating={isUpdating("mute_while_recording")}
        label={t("settings.debug.muteWhileRecording.label")}
        description={t("settings.debug.muteWhileRecording.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  });
