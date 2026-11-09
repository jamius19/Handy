/*
 * Author: Jamius Siam
 * Since: 09/11/2026
 */

import React from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { SettingContainer } from "../ui/SettingContainer";
import { ApiKeyField } from "./PostProcessingSettingsApi/ApiKeyField";
import { CloudModelPicker } from "./CloudModelPicker";
import { useSettings } from "../../hooks/useSettings";
import { VERCEL_LIVE_CLOUD_MODEL_ID } from "../shared/modelTable/tableModel";

/** The provider whose key the gateway models use, shared with post-processing. */
const VERCEL_PROVIDER = "vercel_ai_gateway";

const fetchModels = () => commands.fetchVercelTranscriptionModels(false);
const fetchStreamingModels = () =>
  commands.fetchVercelTranscriptionModels(true);

interface VercelGatewaySettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  /** Which Vercel AI Gateway row is selected: each picks its own model. */
  modelId?: string;
}

/** The AI Gateway API key and the gateway model that transcribes. */
export const VercelGatewaySettings: React.FC<VercelGatewaySettingsProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false, modelId }) => {
    const { t } = useTranslation();
    const { settings, updatePostProcessApiKey, isUpdating } = useSettings();
    const apiKey = settings?.post_process_api_keys?.[VERCEL_PROVIDER] ?? "";
    const isLive = modelId === VERCEL_LIVE_CLOUD_MODEL_ID;

    return (
      <>
        <SettingContainer
          title={t("settings.cloudModel.vercelApiKey.title")}
          description={t("settings.cloudModel.vercelApiKey.description")}
          descriptionMode={descriptionMode}
          layout="horizontal"
          grouped={grouped}
        >
          <ApiKeyField
            value={apiKey}
            onBlur={(value) =>
              updatePostProcessApiKey(VERCEL_PROVIDER, value.trim())
            }
            placeholder={t("settings.cloudModel.vercelApiKey.placeholder")}
            disabled={isUpdating(`post_process_api_key:${VERCEL_PROVIDER}`)}
            className="min-w-[320px]"
          />
        </SettingContainer>
        {isLive && (
          <CloudModelPicker
            key="live"
            setting="vercel_live_transcription_model"
            textKey="settings.cloudModel.vercelLiveModel"
            fetchModels={fetchStreamingModels}
            descriptionMode={descriptionMode}
            grouped={grouped}
          />
        )}
        {/* The live model falls back to the batch model, which streaming
            models can't stand in for. */}
        <CloudModelPicker
          key={isLive ? "fallback" : "batch"}
          setting="vercel_transcription_model"
          textKey={
            isLive
              ? "settings.cloudModel.vercelFallbackModel"
              : "settings.cloudModel.vercelModel"
          }
          fetchModels={fetchModels}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
      </>
    );
  });
