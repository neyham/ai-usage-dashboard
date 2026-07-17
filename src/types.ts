// Mirrors the Rust `UsageSummary` DTO emitted by the backend.
// The renderer only ever receives this sanitized shape — never tokens, keys,
// credential file contents, or raw API error bodies.

export type SummaryStatus = "idle" | "ok" | "refreshing" | "partial" | "error";

export type LaunchMode = "normal" | "fullscreen" | "screensaver";

export interface EnabledProviders {
  codex: boolean;
  claude: boolean;
  deepseek: boolean;
}

export interface ClaudeService {
  status: string;
  fromCache: boolean;
  dataMayBeStale: boolean;
  cooldownUntilLocal?: string;
  fiveHourPercent?: number;
  sevenDayPercent?: number;
  fiveHourResetLocal?: string;
  sevenDayResetLocal?: string;
}

export interface CodexService {
  status: string;
  fromCache: boolean;
  dataMayBeStale: boolean;
  plan?: string;
  fiveHourPercent?: number;
  sevenDayPercent?: number;
  fiveHourResetLocal?: string;
  sevenDayResetLocal?: string;
}

export interface DeepSeekService {
  status: string;
  fromCache: boolean;
  dataMayBeStale: boolean;
  currency?: string;
  balance?: string;
}

export interface Services {
  codex: CodexService;
  claude: ClaudeService;
  deepseek: DeepSeekService;
}

export interface UsageSummary {
  refreshedAt: string | null;
  status: SummaryStatus;
  enabledProviders: EnabledProviders;
  services: Services;
}
