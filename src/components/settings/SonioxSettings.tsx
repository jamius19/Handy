/*
 * Author: Jamius Siam
 * Since: 13/11/2026
 */

import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type Result,
  type SonioxModels,
  type SonioxRegion,
} from "@/bindings";
import { Dropdown, SettingContainer } from "@/components/ui";
import { ApiKeyField } from "./PostProcessingSettingsApi/ApiKeyField";
import { CloudModelPicker } from "./CloudModelPicker";
import { useSettings } from "../../hooks/useSettings";
import { SONIOX_LIVE_CLOUD_MODEL_ID } from "../shared/modelTable/tableModel";

const REGIONS: SonioxRegion[] = ["us", "eu", "jp", "in"];

/** The catalogue request in flight, so pickers that load together share it. */
let pending: Promise<Result<SonioxModels, string>> | null = null;

const fetchCatalogue = () => {
  pending ??= commands.fetchSonioxModels().finally(() => {
    pending = null;
  });
  return pending;
};

const modelsOf = async (
  kind: keyof SonioxModels,
): Promise<Result<string[], string>> => {
  const result = await fetchCatalogue();
  return result.status === "ok"
    ? { status: "ok", data: result.data[kind] }
    : result;
};

const fetchLiveModels = () => modelsOf("live");
const fetchBatchModels = () => modelsOf("batch");

interface SonioxSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  /** Which Soniox row is selected: each picks its own model. */
  modelId?: string;
}

/** The Soniox API key, region and the Soniox model that transcribes. */
export const SonioxSettings: React.FC<SonioxSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false, modelId }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const apiKey = getSetting("soniox_api_key") ?? "";
    const isLive = modelId === SONIOX_LIVE_CLOUD_MODEL_ID;
    // The list needs the key and the region's API, so the pickers load it
    // again once either is saved.
    const [catalogue, setCatalogue] = useState(0);
    const reloadAfter = (saving: Promise<void>) =>
      void saving.then(() => setCatalogue((n) => n + 1));

    return (
      <>
        <SettingContainer
          title={t("settings.cloudModel.sonioxApiKey.title")}
          description={t("settings.cloudModel.sonioxApiKey.description")}
          descriptionMode={descriptionMode}
          layout="horizontal"
          grouped={grouped}
        >
          <ApiKeyField
            value={apiKey}
            onBlur={(value) => {
              if (value.trim() !== apiKey) {
                reloadAfter(updateSetting("soniox_api_key", value.trim()));
              }
            }}
            placeholder={t("settings.cloudModel.sonioxApiKey.placeholder")}
            disabled={isUpdating("soniox_api_key")}
            className="min-w-[320px]"
          />
        </SettingContainer>
        <SettingContainer
          title={t("settings.cloudModel.sonioxRegion.title")}
          description={t("settings.cloudModel.sonioxRegion.description")}
          descriptionMode={descriptionMode}
          layout="horizontal"
          grouped={grouped}
        >
          <Dropdown
            options={REGIONS.map((region) => ({
              value: region,
              label: t(`settings.cloudModel.sonioxRegion.regions.${region}`),
            }))}
            selectedValue={getSetting("soniox_region") ?? "us"}
            onSelect={(value) =>
              reloadAfter(updateSetting("soniox_region", value as SonioxRegion))
            }
            disabled={isUpdating("soniox_region")}
          />
        </SettingContainer>
        {isLive && (
          <CloudModelPicker
            key={`live:${catalogue}`}
            setting="soniox_live_transcription_model"
            textKey="settings.cloudModel.sonioxLiveModel"
            fetchModels={fetchLiveModels}
            descriptionMode={descriptionMode}
            grouped={grouped}
          />
        )}
        {/* The live model falls back to the async model, since a real-time
            model can't run as a job. */}
        <CloudModelPicker
          key={`${isLive ? "fallback" : "async"}:${catalogue}`}
          setting="soniox_transcription_model"
          textKey={
            isLive
              ? "settings.cloudModel.sonioxFallbackModel"
              : "settings.cloudModel.sonioxModel"
          }
          fetchModels={fetchBatchModels}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
      </>
    );
  },
);
