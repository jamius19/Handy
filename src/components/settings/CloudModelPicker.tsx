/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCcw } from "lucide-react";
import type { Result } from "@/bindings";
import { SettingContainer } from "../ui/SettingContainer";
import { ResetButton } from "../ui/ResetButton";
import { ModelSelect } from "./PostProcessingSettingsApi/ModelSelect";
import { useSettings } from "../../hooks/useSettings";
import { notify } from "../../stores/noticeStore";

/** The settings that hold a cloud model's picked provider model id. */
type ModelSettingKey =
  | "openrouter_transcription_model"
  | "vercel_transcription_model"
  | "vercel_live_transcription_model";

interface CloudModelPickerProps {
  setting: ModelSettingKey;
  /** Holds `title`, `description`, `placeholder`, `refresh` and `fetchFailed`. */
  textKey: string;
  /** A stable function, or the list refetches on every render. */
  fetchModels: () => Promise<Result<string[], string>>;
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/** Picks the provider model a cloud model transcribes with, from the
 * provider's list or typed in by id. */
export const CloudModelPicker: React.FC<CloudModelPickerProps> = React.memo(
  ({
    setting,
    textKey,
    fetchModels,
    descriptionMode = "tooltip",
    grouped = false,
  }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const model = getSetting(setting) ?? "";
    const [models, setModels] = useState<string[]>([]);
    const [isFetching, setIsFetching] = useState(false);

    // A failure only notifies when the person asked for the list; the model
    // can still be typed in by id.
    const loadModels = useCallback(
      async (notifyOnError: boolean) => {
        setIsFetching(true);
        try {
          const result = await fetchModels();
          if (result.status === "ok") {
            setModels(result.data);
          } else if (notifyOnError) {
            notify({
              title: t(`${textKey}.fetchFailed`),
              detail: result.error,
            });
          }
        } finally {
          setIsFetching(false);
        }
      },
      [fetchModels, t, textKey],
    );

    useEffect(() => {
      void loadModels(false);
    }, [loadModels]);

    const options = models.map((id) => ({ value: id, label: id }));
    const selectModel = (value: string) => updateSetting(setting, value.trim());

    return (
      <SettingContainer
        title={t(`${textKey}.title`)}
        description={t(`${textKey}.description`)}
        descriptionMode={descriptionMode}
        layout="horizontal"
        grouped={grouped}
      >
        <div className="flex items-center gap-2">
          <ModelSelect
            value={model}
            options={options}
            disabled={isUpdating(setting)}
            isLoading={isFetching}
            placeholder={t(`${textKey}.placeholder`)}
            onSelect={selectModel}
            onCreate={selectModel}
            onBlur={() => {}}
            className="flex-1 min-w-[280px]"
          />
          <ResetButton
            onClick={() => void loadModels(true)}
            disabled={isFetching}
            ariaLabel={t(`${textKey}.refresh`)}
            className="flex h-10 w-10 items-center justify-center"
          >
            <RefreshCcw
              className={`h-4 w-4 ${isFetching ? "animate-spin" : ""}`}
            />
          </ResetButton>
        </div>
      </SettingContainer>
    );
  },
);
