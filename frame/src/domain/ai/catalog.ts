import type {
  WorkKeyStateV1,
  WorkModelEntry,
  WorkModelPrice,
  WorkModelProvider,
  WorkModelRole,
  WorkModelsV1,
} from "$shared/ipc/bindings";

/** Providers that take the person's own key, in the order they are shown. */
export const keyedProviders: readonly WorkModelProvider[] = [
  "anthropic",
  "open_ai",
  "google",
  "xai",
  "mistral",
  "deep_seek",
  "groq",
  "cerebras",
  "open_router",
  "compatible",
];

const names: Record<WorkModelProvider, string> = {
  anthropic: "Anthropic",
  open_ai: "OpenAI",
  google: "Google",
  deep_seek: "DeepSeek",
  open_router: "OpenRouter",
  xai: "xAI",
  mistral: "Mistral",
  groq: "Groq",
  cerebras: "Cerebras",
  compatible: "Custom",
  cloud: "Zephium",
};

export function providerName(provider: WorkModelProvider): string {
  return names[provider];
}

/** The model's name as the trigger shows it: the mark beside it says the family. */
export function shortName(entry: WorkModelEntry): string {
  const name = entry.display_name;
  for (const family of ["Claude ", "DeepSeek "]) {
    if (name.startsWith(family) && name.length > family.length) return name.slice(family.length);
  }
  return name;
}

export function keyState(models: WorkModelsV1 | null, provider: WorkModelProvider): WorkKeyStateV1 {
  return models?.providers.find((row) => row.provider === provider)?.key ?? "missing";
}

/** A provider can run calls now: its key is set and not refused. */
export function usable(models: WorkModelsV1 | null, provider: WorkModelProvider): boolean {
  if (!models) return false;
  if (provider === "cloud")
    return (
      models.cloud.signed_in &&
      models.entries.some(
        (entry) => entry.model.provider === "cloud" && !models.cloud.locked.includes(entry.id),
      )
    );
  if (provider === "compatible")
    return (
      !!models.providers.find((row) => row.provider === "compatible")?.base &&
      keyState(models, provider) !== "invalid"
    );
  const key = keyState(models, provider);
  return key === "set" || key === "valid";
}

type ModelGroup = { provider: WorkModelProvider; entries: WorkModelEntry[] };

/** Without a key, only these appear in the picker; Settings → AI offers every provider. */
const suggested: readonly WorkModelProvider[] = ["anthropic", "open_ai", "google"];

/**
 * The picker's list for a role: every curated model of usable providers,
 * recommended first, grouped by provider, the signed-in account's hosted
 * models first. The person's own server adds the models it serves.
 * The best-known providers without a key are listed apart, as needing one.
 */
export function pickerGroups(
  models: WorkModelsV1 | null,
  role: WorkModelRole,
  endpoint: readonly WorkModelEntry[] = [],
) {
  const entries = [...(models?.entries ?? []), ...endpoint];
  const order: readonly WorkModelProvider[] = ["cloud", ...keyedProviders];
  const ready: ModelGroup[] = [];
  const needsKey: WorkModelProvider[] = [];
  for (const provider of order) {
    const seen = new Set<string>();
    const offered = entries
      .filter((entry) => {
        if (entry.model.provider !== provider || seen.has(entry.id)) return false;
        seen.add(entry.id);
        // Native resolves known IDs from the catalog before provider listings.
        return entry.roles.includes(role) || (role !== "decision" && entry.roles.includes("lead"));
      })
      .sort((a, b) => Number(b.recommended) - Number(a.recommended));
    if (usable(models, provider)) {
      if (offered.length) ready.push({ provider, entries: offered });
    } else if (suggested.includes(provider)) {
      needsKey.push(provider);
    }
  }
  return { ready, needsKey };
}

/** A hosted model the account's plan does not include: listed to explain the plan, never chosen. */
export function locked(models: WorkModelsV1 | null, id: string): boolean {
  return !!models?.cloud.locked.includes(id);
}

/** The plan a locked hosted model needs, as the server names it; null when it does not say. */
export function requiredPlan(models: WorkModelsV1 | null, id: string): string | null {
  return models?.cloud.required.find((row) => row.id === id)?.plan ?? null;
}

export function entryOf(models: WorkModelsV1 | null, id: string | null | undefined) {
  if (!models || !id) return null;
  return models.entries.find((entry) => entry.id === id) ?? null;
}

/** The efforts `id` takes as the lead, weakest first; empty when it takes none. */
export function effortLevels(models: WorkModelsV1 | null, id: string | null | undefined) {
  if (!models || !id) return [];
  return models.efforts.find((row) => row.id === id)?.levels ?? [];
}

/** A context window the way a model card says it: 1M, 400K, 128K. */
export function contextSize(tokens: number): string {
  const binary = tokens % 1024 === 0;
  if (tokens >= 1_000_000) {
    const millions = binary ? tokens / 1_048_576 : Math.floor(tokens / 100_000) / 10;
    return `${millions}M`;
  }
  return `${binary ? tokens / 1024 : Math.round(tokens / 1000)}K`;
}

/** One to four, from what a million output tokens cost; null when unpriced. */
export function priceTier(price: WorkModelPrice | null | undefined): 1 | 2 | 3 | 4 | null {
  if (!price) return null;
  const output = price.output / 1_000_000;
  if (output < 2) return 1;
  if (output < 15) return 2;
  if (output < 40) return 3;
  return 4;
}

/** Dollars per million tokens, as short as the number allows. */
export function dollars(micros: number): string {
  const value = micros / 1_000_000;
  const digits = value >= 1 ? (Number.isInteger(value) ? 0 : 2) : value >= 0.1 ? 2 : 3;
  return `$${value
    .toFixed(digits)
    .replace(/(\.\d*?)0+$/u, "$1")
    .replace(/\.$/u, "")}`;
}

/** The picker's matches for `query` across every ready provider, in the rail's order. */
export function searchEntries(groups: readonly ModelGroup[], query: string): WorkModelEntry[] {
  const words = query.toLocaleLowerCase().split(/\s+/u).filter(Boolean);
  if (!words.length) return [];
  return groups.flatMap((group) =>
    group.entries.filter((entry) => {
      const text =
        `${entry.display_name} ${providerName(group.provider)} ${entry.model.model}`.toLocaleLowerCase();
      return words.every((word) => text.includes(word));
    }),
  );
}
