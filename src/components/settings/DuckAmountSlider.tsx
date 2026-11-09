import React from "react";
import { useTranslation } from "react-i18next";
import { Slider } from "../ui/Slider";
import { useSettings } from "../../hooks/useSettings";

export const DuckAmountSlider: React.FC<{ disabled?: boolean }> = ({
  disabled = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting } = useSettings();
  const duckAmount = getSetting("duck_amount") ?? 50;

  return (
    <Slider
      value={duckAmount}
      onChange={(value: number) => updateSetting("duck_amount", value)}
      min={0}
      max={100}
      step={1}
      label={t("settings.debug.duckAmount.label")}
      description={t("settings.debug.duckAmount.description")}
      descriptionMode="tooltip"
      grouped
      formatValue={(value) => `${Math.round(value)}%`}
      disabled={disabled}
    />
  );
};
