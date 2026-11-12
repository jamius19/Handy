import type { TFunction } from "i18next";
import { catalog } from "@handy-computer/models";
import type { CloudProvider, ModelInfo } from "@/bindings";
import { repoIdOf } from "@/lib/models/retired";
import {
  getTranslatedModelDescription,
  getTranslatedModelName,
} from "@/lib/utils/modelTranslation";
import type { TableModel } from "./context";

// Legacy (Url-sourced .bin) models still run from disk but aren't offered.
export const isLegacyModel = (model: ModelInfo): boolean =>
  typeof model.source === "object" && "Url" in model.source;

/** The API a cloud model runs on, or null for a local model. */
export const cloudProviderOf = (model: ModelInfo): CloudProvider | null =>
  typeof model.source === "object" && "Cloud" in model.source
    ? model.source.Cloud.provider
    : null;

/** Runs on a cloud API: nothing to download or delete. */
export const isCloudModel = (model: ModelInfo): boolean =>
  cloudProviderOf(model) !== null;

/** The OpenAI cloud model's id (`OPENAI_CLOUD_MODEL_ID` in Rust). */
export const CLOUD_MODEL_ID = "openai-gpt-transcribe";

/** The streaming OpenAI cloud model's id (`OPENAI_LIVE_CLOUD_MODEL_ID` in Rust). */
export const LIVE_CLOUD_MODEL_ID = "openai-gpt-live-transcribe";

/** The streaming Vercel AI Gateway model's id (`VERCEL_LIVE_CLOUD_MODEL_ID` in Rust). */
export const VERCEL_LIVE_CLOUD_MODEL_ID = "vercel-ai-gateway-live";

/** The streaming Soniox model's id (`SONIOX_LIVE_CLOUD_MODEL_ID` in Rust). */
export const SONIOX_LIVE_CLOUD_MODEL_ID = "soniox-live";

// Extract a GGUF quantization label from a filename, if present (e.g. "Q8_0").
const quantOf = (filename: string): string | null => {
  const match = filename.match(
    /[._-](IQ\d+_\w+|Q\d+(?:_\w+)?|F16|BF16|F32)\.gguf$/i,
  );
  return match ? match[1].toUpperCase() : null;
};

const CATALOG_BY_REPO = new Map(
  catalog.models
    .filter((m) => m.role === "asr")
    .map((m) => [m.repository, m] as const),
);

/** An app model as the table shows it, joined to its catalog entry by repo. */
export const toTableModel = (info: ModelInfo, t: TFunction): TableModel => {
  const repo = repoIdOf(info);
  const model = info.is_custom || !repo ? undefined : CATALOG_BY_REPO.get(repo);
  return {
    id: info.id,
    name: getTranslatedModelName(info, t),
    description: getTranslatedModelDescription(info, t),
    languages: info.supported_languages,
    streaming: info.supports_streaming,
    translate: info.supports_translation,
    sizeMb: Number(info.size_mb),
    // The file's own, so a different quant found on disk says so.
    quant: quantOf(info.filename) ?? model?.quant ?? null,
    kind: isCloudModel(info)
      ? "cloud"
      : info.is_custom
        ? "custom"
        : isLegacyModel(info)
          ? "legacy"
          : model?.curated
            ? "curated"
            : "catalog",
    model,
  };
};
