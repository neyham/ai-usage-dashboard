import type { EnabledProviders, Services } from "./types";

export const PROVIDER_ORDER = [
  "codex",
  "claude",
  "deepseek",
  "grok",
  "cursor",
  "antigravity",
] as const;

export type ProviderKind = (typeof PROVIDER_ORDER)[number];
export type PlanSize = "L" | "M" | "S";
export type LayoutMode = "equal" | "five-hero" | "five-split" | "five-fill";

export interface GridSlot {
  column: string;
  row: string;
}

export interface StoredPlan {
  plan: string | null;
  size: PlanSize;
}

export type StoredPlanSizes = Partial<Record<ProviderKind, StoredPlan>>;

const SIZE_RANK: Record<PlanSize, number> = { L: 2, M: 1, S: 0 };
const STORAGE_KEY = "ai-usage-dashboard.plan-sizes.v1";

export function storageKey(judgeDemo: boolean): string {
  return judgeDemo ? `${STORAGE_KEY}.judge-demo` : STORAGE_KEY;
}

export function enabledProviderList(enabled: EnabledProviders): ProviderKind[] {
  return PROVIDER_ORDER.filter((kind) => enabled[kind]);
}

export function livePlanOf(kind: ProviderKind, services: Services): string | undefined {
  if (kind === "claude" || kind === "deepseek") {
    return undefined;
  }
  const plan = services[kind].plan;
  return typeof plan === "string" ? plan : undefined;
}

export function sizeFromPlan(kind: ProviderKind, plan: string | undefined): PlanSize {
  if (kind === "deepseek") {
    return "S";
  }
  if (kind === "claude") {
    return "M";
  }

  const normalized = plan?.trim().toLowerCase() ?? "";
  if (!normalized) {
    return "M";
  }
  if (
    normalized === "free" ||
    normalized === "hobby" ||
    normalized === "free-tier" ||
    normalized === "free tier"
  ) {
    return "S";
  }

  if (kind === "grok") {
    if (
      normalized === "supergrok heavy" ||
      normalized === "supergrokpro" ||
      normalized === "supergrok_heavy"
    ) {
      return "L";
    }
    return "M";
  }

  if (kind === "cursor") {
    if (normalized === "ultra" || normalized === "enterprise") {
      return "L";
    }
    return "M";
  }

  if (kind === "antigravity") {
    if (
      normalized === "google ai ultra" ||
      normalized === "ai ultra" ||
      normalized === "ultra"
    ) {
      return "L";
    }
    return "M";
  }

  return "M";
}

export function resolveSize(
  kind: ProviderKind,
  livePlan: string | undefined,
  stored: StoredPlanSizes,
): { size: PlanSize; next?: StoredPlan } {
  if (kind === "deepseek") {
    return { size: "S" };
  }
  if (kind === "claude") {
    return { size: "M" };
  }

  const previous = stored[kind];
  const live = livePlan?.trim() ?? "";
  if (live) {
    const liveSize = sizeFromPlan(kind, live);
    if (!previous || previous.plan == null) {
      return { size: liveSize, next: { plan: live, size: liveSize } };
    }
    if (previous.plan !== live) {
      return { size: liveSize, next: { plan: live, size: liveSize } };
    }
    return { size: previous.size };
  }

  if (previous) {
    return { size: previous.size };
  }
  return { size: sizeFromPlan(kind, undefined) };
}

function rankProviders(kinds: ProviderKind[], sizes: Record<ProviderKind, PlanSize>): ProviderKind[] {
  return [...kinds].sort((left, right) => {
    const delta = SIZE_RANK[sizes[right]] - SIZE_RANK[sizes[left]];
    if (delta !== 0) {
      return delta;
    }
    return PROVIDER_ORDER.indexOf(left) - PROVIDER_ORDER.indexOf(right);
  });
}

function byProviderOrder(left: ProviderKind, right: ProviderKind): number {
  return PROVIDER_ORDER.indexOf(left) - PROVIDER_ORDER.indexOf(right);
}

export function computePanelLayout(
  enabled: ProviderKind[],
  sizes: Record<ProviderKind, PlanSize>,
): {
  mode: LayoutMode;
  placements: Partial<Record<ProviderKind, GridSlot>>;
  slots: Partial<Record<ProviderKind, string>>;
} {
  if (enabled.length !== 5) {
    return { mode: "equal", placements: {}, slots: {} };
  }

  const largeCount = enabled.filter((kind) => sizes[kind] === "L").length;
  if (largeCount === 1) {
    const hero = enabled.find((kind) => sizes[kind] === "L");
    if (!hero) {
      return { mode: "equal", placements: {}, slots: {} };
    }
    const rest = rankProviders(
      enabled.filter((kind) => kind !== hero),
      sizes,
    );
    return {
      mode: "five-hero",
      placements: {
        [hero]: { column: "1 / 7", row: "1 / 3" },
        [rest[0]]: { column: "7 / 10", row: "1 / 2" },
        [rest[1]]: { column: "10 / 13", row: "1 / 2" },
        [rest[2]]: { column: "7 / 10", row: "2 / 3" },
        [rest[3]]: { column: "10 / 13", row: "2 / 3" },
      },
      slots: {
        [hero]: "hero",
        [rest[0]]: "a",
        [rest[1]]: "b",
        [rest[2]]: "c",
        [rest[3]]: "d",
      },
    };
  }

  if (largeCount === 2) {
    const large = enabled.filter((kind) => sizes[kind] === "L").sort(byProviderOrder);
    const rest = rankProviders(
      enabled.filter((kind) => sizes[kind] !== "L"),
      sizes,
    );
    return {
      mode: "five-split",
      placements: {
        [large[0]]: { column: "1 / 7", row: "1 / 2" },
        [large[1]]: { column: "7 / 13", row: "1 / 2" },
        [rest[0]]: { column: "1 / 5", row: "2 / 3" },
        [rest[1]]: { column: "5 / 9", row: "2 / 3" },
        [rest[2]]: { column: "9 / 13", row: "2 / 3" },
      },
      slots: {
        [large[0]]: "top-left",
        [large[1]]: "top-right",
        [rest[0]]: "a",
        [rest[1]]: "b",
        [rest[2]]: "c",
      },
    };
  }

  return {
    mode: "five-fill",
    placements: {
      [enabled[0]]: { column: "1 / 5", row: "1 / 2" },
      [enabled[1]]: { column: "5 / 9", row: "1 / 2" },
      [enabled[2]]: { column: "9 / 13", row: "1 / 2" },
      [enabled[3]]: { column: "1 / 7", row: "2 / 3" },
      [enabled[4]]: { column: "7 / 13", row: "2 / 3" },
    },
    slots: {
      [enabled[0]]: "a",
      [enabled[1]]: "b",
      [enabled[2]]: "c",
      [enabled[3]]: "d",
      [enabled[4]]: "e",
    },
  };
}

export function loadStoredPlanSizes(judgeDemo: boolean): StoredPlanSizes {
  try {
    const raw = localStorage.getItem(storageKey(judgeDemo));
    if (!raw) {
      return {};
    }
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") {
      return {};
    }
    const stored: StoredPlanSizes = {};
    for (const kind of PROVIDER_ORDER) {
      const entry = (parsed as Record<string, unknown>)[kind];
      if (!entry || typeof entry !== "object") {
        continue;
      }
      const plan = (entry as { plan?: unknown }).plan;
      const size = (entry as { size?: unknown }).size;
      if ((plan === null || typeof plan === "string") && (size === "L" || size === "M" || size === "S")) {
        stored[kind] = { plan, size };
      }
    }
    return stored;
  } catch {
    return {};
  }
}

export function saveStoredPlanSizes(judgeDemo: boolean, value: StoredPlanSizes): void {
  try {
    localStorage.setItem(storageKey(judgeDemo), JSON.stringify(value));
  } catch {
    // Private mode or a full store should not break the dashboard.
  }
}

export function mergeStoredPlanSizes(
  current: StoredPlanSizes,
  updates: StoredPlanSizes,
): StoredPlanSizes {
  return { ...current, ...updates };
}
