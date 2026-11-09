/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCcw } from "lucide-react";
import { commands } from "@/bindings";
import { SettingContainer } from "../ui/SettingContainer";
import { ResetButton } from "../ui/ResetButton";
import { ApiKeyField } from "./PostProcessingSettingsApi/ApiKeyField";
import { ModelSelect } from "./PostProcessingSettingsApi/ModelSelect";
import { useSettings } from "../../hooks/useSettings";
import { notify } from "../../stores/noticeStore";

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
    const {
      settings,
      getSetting,
      updateSetting,
      updatePostProcessApiKey,
      isUpdating,
    } = useSettings();
    const apiKey = settings?.post_process_api_keys?.[OPENROUTER_PROVIDER] ?? "";
    const model = getSetting("openrouter_transcription_model") ?? "";
    const [models, setModels] = useState<string[]>([]);
    const [isFetching, setIsFetching] = useState(false);

    // A failure only notifies when the person asked for the list; the model
    // can still be typed in by id.
    const fetchModels = useCallback(
      async (notifyOnError: boolean) => {
        setIsFetching(true);
        try {
          const result = await commands.fetchOpenrouterTranscriptionModels();
          if (result.status === "ok") {
            setModels(result.data);
          } else if (notifyOnError) {
            notify({
              title: t("settings.cloudModel.openRouterModel.fetchFailed"),
              detail: result.error,
            });
          }
        } finally {
          setIsFetching(false);
        }
      },
      [t],
    );

    useEffect(() => {
      void fetchModels(false);
    }, [fetchModels]);

    const options = models.map((id) => ({ value: id, label: id }));
    const selectModel = (value: string) =>
      updateSetting("openrouter_transcription_model", value.trim());

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
        <SettingContainer
          title={t("settings.cloudModel.openRouterModel.title")}
          description={t("settings.cloudModel.openRouterModel.description")}
          descriptionMode={descriptionMode}
          layout="horizontal"
          grouped={grouped}
        >
          <div className="flex items-center gap-2">
            <ModelSelect
              value={model}
              options={options}
              disabled={isUpdating("openrouter_transcription_model")}
              isLoading={isFetching}
              placeholder={t("settings.cloudModel.openRouterModel.placeholder")}
              onSelect={selectModel}
              onCreate={selectModel}
              onBlur={() => {}}
              className="flex-1 min-w-[280px]"
            />
            <ResetButton
              onClick={() => void fetchModels(true)}
              disabled={isFetching}
              ariaLabel={t("settings.cloudModel.openRouterModel.refresh")}
              className="flex h-10 w-10 items-center justify-center"
            >
              <RefreshCcw
                className={`h-4 w-4 ${isFetching ? "animate-spin" : ""}`}
              />
            </ResetButton>
          </div>
        </SettingContainer>
      </>
    );
  },
);
