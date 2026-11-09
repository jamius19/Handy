import React, { useEffect, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { ChevronRight, RefreshCcw } from "lucide-react";
import type { TFunction } from "i18next";
import {
  commands,
  events,
  type OpenAiServiceTier,
  type PostProcessApiFormat,
  type PostProcessLatency,
  type ReasoningLevel,
} from "@/bindings";

import { Alert } from "../../ui/Alert";
import {
  Dropdown,
  SettingContainer,
  SettingsGroup,
  Textarea,
} from "@/components/ui";
import { Button } from "../../ui/Button";
import { ResetButton } from "../../ui/ResetButton";
import { Input } from "../../ui/Input";

import { ProviderSelect } from "../PostProcessingSettingsApi/ProviderSelect";
import { BaseUrlField } from "../PostProcessingSettingsApi/BaseUrlField";
import { ApiKeyField } from "../PostProcessingSettingsApi/ApiKeyField";
import { ModelSelect } from "../PostProcessingSettingsApi/ModelSelect";
import { usePostProcessProviderState } from "../PostProcessingSettingsApi/usePostProcessProviderState";
import { useSettings } from "../../../hooks/useSettings";

const REASONING_LEVELS: ReasoningLevel[] = ["off", "low", "medium", "high"];
const OPENAI_SERVICE_TIERS: OpenAiServiceTier[] = ["default", "fast"];
const OPENROUTER_PROVIDER_PLACEHOLDER =
  '{ "order": ["anthropic"], "allow_fallbacks": false }';

/** P75 and P90 times per provider and model, reloaded after each dictation. */
const usePostProcessLatencies = (): PostProcessLatency[] => {
  const [latencies, setLatencies] = useState<PostProcessLatency[]>([]);

  useEffect(() => {
    const load = async () => {
      const result = await commands.getPostProcessLatencies();
      if (result.status === "ok") setLatencies(result.data);
    };
    load();
    // A history entry is saved right after its post-processing time.
    const unlisten = events.historyUpdatePayload.listen(() => {
      load();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return latencies;
};

const formatLatency = (t: TFunction, ms: number): string =>
  ms < 1000
    ? t("settings.postProcessing.latency.milliseconds", { value: ms })
    : t("settings.postProcessing.latency.seconds", {
        value: (ms / 1000).toFixed(2),
      });

/** Empty or `null` clears the setting; anything else must be a JSON object. */
const validateProviderRouting = (
  text: string,
): "invalidJson" | "notObject" | null => {
  const trimmed = text.trim();
  if (trimmed === "" || trimmed === "null") return null;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    return typeof parsed === "object" &&
      parsed !== null &&
      !Array.isArray(parsed)
      ? null
      : "notObject";
  } catch {
    return "invalidJson";
  }
};

const OpenRouterProviderRouting: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const stored = getSetting("post_process_openrouter_provider") ?? null;
  const [isExpanded, setIsExpanded] = useState(false);
  const [draft, setDraft] = useState(stored ?? "");

  useEffect(() => {
    setDraft(stored ?? "");
  }, [stored]);

  const error = validateProviderRouting(draft);

  const handleBlur = () => {
    if (error) return;
    const trimmed = draft.trim();
    const value = trimmed === "" || trimmed === "null" ? null : trimmed;
    if (value !== stored) {
      updateSetting("post_process_openrouter_provider", value);
    }
  };

  return (
    <div>
      <SettingContainer
        title={t("settings.postProcessing.api.openrouterProvider.title")}
        description={t(
          "settings.postProcessing.api.openrouterProvider.description",
        )}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <Button
          onClick={() => setIsExpanded((expanded) => !expanded)}
          variant="secondary"
          size="sm"
          aria-expanded={isExpanded}
          className="flex items-center gap-1"
        >
          <ChevronRight
            className={`h-3 w-3 transition-transform ${isExpanded ? "rotate-90" : ""}`}
          />
          {isExpanded
            ? t("settings.postProcessing.api.openrouterProvider.hide")
            : t("settings.postProcessing.api.openrouterProvider.show")}
        </Button>
      </SettingContainer>
      {isExpanded && (
        <div className="px-4 pb-3 flex flex-col gap-2">
          <Textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={handleBlur}
            placeholder={OPENROUTER_PROVIDER_PLACEHOLDER}
            disabled={isUpdating("post_process_openrouter_provider")}
            spellCheck={false}
            className={`font-mono ${error ? "border-error hover:border-error focus:border-error" : ""}`}
          />
          {error && (
            <p className="text-xs text-error">
              {t(`settings.postProcessing.api.openrouterProvider.${error}`)}
            </p>
          )}
        </div>
      )}
    </div>
  );
};

const PostProcessingSettingsApiComponent: React.FC = () => {
  const { t } = useTranslation();
  const state = usePostProcessProviderState();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const latencies = usePostProcessLatencies();
  const currentLatency = latencies.find(
    (latency) =>
      latency.provider_id === state.selectedProviderId &&
      latency.model === state.model,
  );

  return (
    <>
      <SettingContainer
        title={t("settings.postProcessing.api.provider.title")}
        description={t("settings.postProcessing.api.provider.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <div className="flex items-center gap-2">
          <ProviderSelect
            options={state.providerOptions}
            value={state.selectedProviderId}
            onChange={state.handleProviderSelect}
          />
        </div>
      </SettingContainer>

      {state.isAppleProvider ? (
        state.appleIntelligenceUnavailable ? (
          <Alert variant="error" contained>
            {t("settings.postProcessing.api.appleIntelligence.unavailable")}
          </Alert>
        ) : null
      ) : (
        <>
          {state.isCustomProvider && (
            <SettingContainer
              title={t("settings.postProcessing.api.apiFormat.title")}
              description={t(
                "settings.postProcessing.api.apiFormat.description",
              )}
              descriptionMode="tooltip"
              layout="horizontal"
              grouped={true}
            >
              <Dropdown
                options={[
                  {
                    value: "openai",
                    label: t(
                      "settings.postProcessing.api.apiFormat.options.openai",
                    ),
                  },
                  {
                    value: "anthropic",
                    label: t(
                      "settings.postProcessing.api.apiFormat.options.anthropic",
                    ),
                  },
                ]}
                selectedValue={state.apiFormat}
                onSelect={(value) =>
                  state.handleApiFormatChange(value as PostProcessApiFormat)
                }
                disabled={state.isApiFormatUpdating}
              />
            </SettingContainer>
          )}

          {state.selectedProvider?.id === "custom" && (
            <SettingContainer
              title={t("settings.postProcessing.api.baseUrl.title")}
              description={t("settings.postProcessing.api.baseUrl.description")}
              descriptionMode="tooltip"
              layout="horizontal"
              grouped={true}
            >
              <div className="flex items-center gap-2">
                <BaseUrlField
                  value={state.baseUrl}
                  onBlur={state.handleBaseUrlChange}
                  placeholder={
                    state.apiFormat === "anthropic"
                      ? t(
                          "settings.postProcessing.api.baseUrl.placeholderAnthropic",
                        )
                      : t("settings.postProcessing.api.baseUrl.placeholder")
                  }
                  disabled={state.isBaseUrlUpdating}
                  className="min-w-[380px]"
                />
              </div>
            </SettingContainer>
          )}

          <SettingContainer
            title={t("settings.postProcessing.api.apiKey.title")}
            description={t("settings.postProcessing.api.apiKey.description")}
            descriptionMode="tooltip"
            layout="horizontal"
            grouped={true}
          >
            <div className="flex items-center gap-2">
              <ApiKeyField
                value={state.apiKey}
                onBlur={state.handleApiKeyChange}
                placeholder={t(
                  "settings.postProcessing.api.apiKey.placeholder",
                )}
                disabled={state.isApiKeyUpdating}
                className="min-w-[320px]"
              />
            </div>
          </SettingContainer>
        </>
      )}

      {!state.isAppleProvider && (
        <SettingContainer
          title={t("settings.postProcessing.api.model.title")}
          description={
            state.isCustomProvider
              ? t("settings.postProcessing.api.model.descriptionCustom")
              : t("settings.postProcessing.api.model.descriptionDefault")
          }
          descriptionMode="tooltip"
          layout="stacked"
          grouped={true}
        >
          <div className="flex items-center gap-2">
            <ModelSelect
              value={state.model}
              options={state.modelOptions}
              disabled={state.isModelUpdating}
              isLoading={state.isFetchingModels}
              placeholder={
                state.modelOptions.length > 0
                  ? t(
                      "settings.postProcessing.api.model.placeholderWithOptions",
                    )
                  : t("settings.postProcessing.api.model.placeholderNoOptions")
              }
              onSelect={state.handleModelSelect}
              onCreate={state.handleModelCreate}
              onBlur={() => {}}
              className="flex-1 min-w-[380px]"
            />
            <ResetButton
              onClick={state.handleRefreshModels}
              disabled={state.isFetchingModels}
              ariaLabel={t("settings.postProcessing.api.model.refreshModels")}
              className="flex h-10 w-10 items-center justify-center"
            >
              <RefreshCcw
                className={`h-4 w-4 ${state.isFetchingModels ? "animate-spin" : ""}`}
              />
            </ResetButton>
          </div>
        </SettingContainer>
      )}

      {!state.isAppleProvider && (
        <SettingContainer
          title={t("settings.postProcessing.api.reasoning.title")}
          description={t("settings.postProcessing.api.reasoning.description")}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <Dropdown
            options={REASONING_LEVELS.map((level) => ({
              value: level,
              label: t(`settings.postProcessing.api.reasoning.levels.${level}`),
            }))}
            selectedValue={state.reasoning}
            onSelect={(value) =>
              state.handleReasoningChange(value as ReasoningLevel)
            }
            disabled={state.isReasoningUpdating}
          />
        </SettingContainer>
      )}

      {state.selectedProviderId === "openai" && (
        <SettingContainer
          title={t("settings.postProcessing.api.serviceTier.title")}
          description={t("settings.postProcessing.api.serviceTier.description")}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <Dropdown
            options={OPENAI_SERVICE_TIERS.map((tier) => ({
              value: tier,
              label: t(`settings.postProcessing.api.serviceTier.tiers.${tier}`),
            }))}
            selectedValue={
              getSetting("post_process_openai_service_tier") ?? "default"
            }
            onSelect={(value) =>
              updateSetting(
                "post_process_openai_service_tier",
                value as OpenAiServiceTier,
              )
            }
            disabled={isUpdating("post_process_openai_service_tier")}
          />
        </SettingContainer>
      )}

      {state.selectedProviderId === "openrouter" && (
        <OpenRouterProviderRouting />
      )}

      <SettingContainer
        title={t("settings.postProcessing.api.latency.title")}
        description={t("settings.postProcessing.api.latency.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <span className="text-sm tabular-nums">
          {currentLatency
            ? t("settings.postProcessing.api.latency.value", {
                p75: formatLatency(t, currentLatency.p75_ms),
                p90: formatLatency(t, currentLatency.p90_ms),
                requests: t("settings.postProcessing.latency.requestCount", {
                  count: currentLatency.samples,
                }),
              })
            : t("settings.postProcessing.api.latency.noData")}
        </span>
      </SettingContainer>
    </>
  );
};

const PostProcessingSettingsPromptsComponent: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating, refreshSettings } =
    useSettings();
  const [isCreating, setIsCreating] = useState(false);
  const [draftName, setDraftName] = useState("");
  const [draftText, setDraftText] = useState("");

  const prompts = getSetting("post_process_prompts") || [];
  const selectedPromptId = getSetting("post_process_selected_prompt_id") || "";
  const selectedPrompt =
    prompts.find((prompt) => prompt.id === selectedPromptId) || null;

  useEffect(() => {
    if (isCreating) return;

    if (selectedPrompt) {
      setDraftName(selectedPrompt.name);
      setDraftText(selectedPrompt.prompt);
    } else {
      setDraftName("");
      setDraftText("");
    }
  }, [
    isCreating,
    selectedPromptId,
    selectedPrompt?.name,
    selectedPrompt?.prompt,
  ]);

  const handlePromptSelect = (promptId: string | null) => {
    if (!promptId) return;
    updateSetting("post_process_selected_prompt_id", promptId);
    setIsCreating(false);
  };

  const handleCreatePrompt = async () => {
    if (!draftName.trim() || !draftText.trim()) return;

    try {
      const result = await commands.addPostProcessPrompt(
        draftName.trim(),
        draftText.trim(),
      );
      if (result.status === "ok") {
        await refreshSettings();
        updateSetting("post_process_selected_prompt_id", result.data.id);
        setIsCreating(false);
      }
    } catch (error) {
      console.error("Failed to create prompt:", error);
    }
  };

  const handleUpdatePrompt = async () => {
    if (!selectedPromptId || !draftName.trim() || !draftText.trim()) return;

    try {
      await commands.updatePostProcessPrompt(
        selectedPromptId,
        draftName.trim(),
        draftText.trim(),
      );
      await refreshSettings();
    } catch (error) {
      console.error("Failed to update prompt:", error);
    }
  };

  const handleDeletePrompt = async (promptId: string) => {
    if (!promptId) return;

    try {
      await commands.deletePostProcessPrompt(promptId);
      await refreshSettings();
      setIsCreating(false);
    } catch (error) {
      console.error("Failed to delete prompt:", error);
    }
  };

  const handleCancelCreate = () => {
    setIsCreating(false);
    if (selectedPrompt) {
      setDraftName(selectedPrompt.name);
      setDraftText(selectedPrompt.prompt);
    } else {
      setDraftName("");
      setDraftText("");
    }
  };

  const handleStartCreate = () => {
    setIsCreating(true);
    setDraftName("");
    setDraftText("");
  };

  const hasPrompts = prompts.length > 0;
  const isDirty =
    !!selectedPrompt &&
    (draftName.trim() !== selectedPrompt.name ||
      draftText.trim() !== selectedPrompt.prompt.trim());

  return (
    <SettingContainer
      title={t("settings.postProcessing.prompts.selectedPrompt.title")}
      description={t(
        "settings.postProcessing.prompts.selectedPrompt.description",
      )}
      descriptionMode="tooltip"
      layout="stacked"
      grouped={true}
    >
      <div className="space-y-3">
        <div className="flex gap-2 min-w-0">
          <Dropdown
            selectedValue={selectedPromptId || null}
            options={prompts.map((p) => ({
              value: p.id,
              label: p.name,
            }))}
            onSelect={(value) => handlePromptSelect(value)}
            placeholder={
              prompts.length === 0
                ? t("settings.postProcessing.prompts.noPrompts")
                : t("settings.postProcessing.prompts.selectPrompt")
            }
            disabled={
              isUpdating("post_process_selected_prompt_id") || isCreating
            }
            className="flex-1 min-w-0"
          />
          <Button
            onClick={handleStartCreate}
            variant="primary"
            size="md"
            disabled={isCreating}
            className="shrink-0"
          >
            {t("settings.postProcessing.prompts.createNew")}
          </Button>
        </div>

        {!isCreating && hasPrompts && selectedPrompt && (
          <div className="space-y-3">
            <div className="space-y-2 flex flex-col">
              <label className="text-sm font-semibold">
                {t("settings.postProcessing.prompts.promptLabel")}
              </label>
              <Input
                type="text"
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
                placeholder={t(
                  "settings.postProcessing.prompts.promptLabelPlaceholder",
                )}
                variant="compact"
              />
            </div>

            <div className="space-y-2 flex flex-col">
              <label className="text-sm font-semibold">
                {t("settings.postProcessing.prompts.promptInstructions")}
              </label>
              <Textarea
                value={draftText}
                onChange={(e) => setDraftText(e.target.value)}
                placeholder={t(
                  "settings.postProcessing.prompts.promptInstructionsPlaceholder",
                )}
              />
              <p className="text-xs text-ink-3">
                <Trans
                  i18nKey="settings.postProcessing.prompts.promptTip"
                  components={{ code: <code /> }}
                />
              </p>
            </div>

            <div className="flex gap-2 pt-2">
              <Button
                onClick={handleUpdatePrompt}
                variant="primary"
                size="md"
                disabled={!draftName.trim() || !draftText.trim() || !isDirty}
              >
                {t("settings.postProcessing.prompts.updatePrompt")}
              </Button>
              <Button
                onClick={() => handleDeletePrompt(selectedPromptId)}
                variant="secondary"
                size="md"
                disabled={!selectedPromptId || prompts.length <= 1}
              >
                {t("settings.postProcessing.prompts.deletePrompt")}
              </Button>
            </div>
          </div>
        )}

        {!isCreating && !selectedPrompt && (
          <div className="p-3 bg-wash rounded-md border border-line">
            <p className="text-sm text-ink-3">
              {hasPrompts
                ? t("settings.postProcessing.prompts.selectToEdit")
                : t("settings.postProcessing.prompts.createFirst")}
            </p>
          </div>
        )}

        {isCreating && (
          <div className="space-y-3">
            <div className="space-y-2 block flex flex-col">
              <label className="text-sm font-semibold text-text">
                {t("settings.postProcessing.prompts.promptLabel")}
              </label>
              <Input
                type="text"
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
                placeholder={t(
                  "settings.postProcessing.prompts.promptLabelPlaceholder",
                )}
                variant="compact"
              />
            </div>

            <div className="space-y-2 flex flex-col">
              <label className="text-sm font-semibold">
                {t("settings.postProcessing.prompts.promptInstructions")}
              </label>
              <Textarea
                value={draftText}
                onChange={(e) => setDraftText(e.target.value)}
                placeholder={t(
                  "settings.postProcessing.prompts.promptInstructionsPlaceholder",
                )}
              />
              <p className="text-xs text-ink-3">
                <Trans
                  i18nKey="settings.postProcessing.prompts.promptTip"
                  components={{ code: <code /> }}
                />
              </p>
            </div>

            <div className="flex gap-2 pt-2">
              <Button
                onClick={handleCreatePrompt}
                variant="primary"
                size="md"
                disabled={!draftName.trim() || !draftText.trim()}
              >
                {t("settings.postProcessing.prompts.createPrompt")}
              </Button>
              <Button
                onClick={handleCancelCreate}
                variant="secondary"
                size="md"
              >
                {t("settings.postProcessing.prompts.cancel")}
              </Button>
            </div>
          </div>
        )}
      </div>
    </SettingContainer>
  );
};

const PostProcessingLatencyTable: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const latencies = usePostProcessLatencies();
  const providers = getSetting("post_process_providers") ?? [];
  const providerLabel = (id: string) =>
    providers.find((provider) => provider.id === id)?.label ?? id;

  if (latencies.length === 0) {
    return (
      <p className="px-4 py-3 text-sm text-ink-3">
        {t("settings.postProcessing.latency.empty")}
      </p>
    );
  }

  return (
    <div className="px-4 py-2 overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="text-start text-xs text-ink-3">
            <th className="py-1 pe-4 text-start font-medium">
              {t("settings.postProcessing.latency.provider")}
            </th>
            <th className="py-1 pe-4 text-start font-medium">
              {t("settings.postProcessing.latency.model")}
            </th>
            <th className="py-1 pe-4 text-end font-medium">
              {t("settings.postProcessing.latency.p75")}
            </th>
            <th className="py-1 pe-4 text-end font-medium">
              {t("settings.postProcessing.latency.p90")}
            </th>
            <th className="py-1 text-end font-medium">
              {t("settings.postProcessing.latency.requests")}
            </th>
          </tr>
        </thead>
        <tbody className="divide-y divide-line">
          {latencies.map((latency) => (
            <tr key={`${latency.provider_id}/${latency.model}`}>
              <td className="py-1.5 pe-4">
                {providerLabel(latency.provider_id)}
              </td>
              <td className="py-1.5 pe-4 font-mono text-xs break-all">
                {latency.model}
              </td>
              <td className="py-1.5 pe-4 text-end tabular-nums">
                {formatLatency(t, latency.p75_ms)}
              </td>
              <td className="py-1.5 pe-4 text-end tabular-nums">
                {formatLatency(t, latency.p90_ms)}
              </td>
              <td className="py-1.5 text-end tabular-nums">
                {latency.samples}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
};

export const PostProcessingSettingsApi = React.memo(
  PostProcessingSettingsApiComponent,
);
PostProcessingSettingsApi.displayName = "PostProcessingSettingsApi";

export const PostProcessingSettingsPrompts = React.memo(
  PostProcessingSettingsPromptsComponent,
);
PostProcessingSettingsPrompts.displayName = "PostProcessingSettingsPrompts";

export const PostProcessingSettings: React.FC = () => {
  const { t } = useTranslation();

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.postProcessing.api.title")}>
        <PostProcessingSettingsApi />
      </SettingsGroup>

      <SettingsGroup title={t("settings.postProcessing.prompts.title")}>
        <PostProcessingSettingsPrompts />
      </SettingsGroup>

      <SettingsGroup
        title={t("settings.postProcessing.latency.title")}
        description={t("settings.postProcessing.latency.description")}
      >
        <PostProcessingLatencyTable />
      </SettingsGroup>
    </div>
  );
};
