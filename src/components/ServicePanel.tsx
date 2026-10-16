import {
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react";
import type {
  AntigravityService,
  ClaudeService,
  CodexService,
  CursorService,
  DeepSeekService,
  GrokService,
  OpenCodeGoService,
} from "../types";
import type { GridSlot, PlanSize } from "../planLayout";
import { IP_THEME, ipVars, type ArtSkin } from "../ipTheme";
import { cornerMascotSrc, mascotMood, mascotSrc, peakPercent } from "../mascot";
import { RingGauge, type RingWindow } from "./RingGauge";
import { StatusChip } from "./StatusChip";
import { CornerBrackets } from "./CornerBrackets";
import { UsageBars } from "./UsageBars";

type Kind = "codex" | "claude" | "deepseek" | "grok" | "cursor" | "antigravity" | "opencodego";

type AnyService = (
  | CodexService
  | ClaudeService
  | DeepSeekService
  | GrokService
  | CursorService
  | AntigravityService
  | OpenCodeGoService
) & {
  plan?: string;
  cooldownUntilLocal?: string;
  fiveHourPercent?: number;
  sevenDayPercent?: number;
  fiveHourResetLocal?: string;
  sevenDayResetLocal?: string;
  extraUsagePercent?: number;
  resetCreditsAvailable?: number;
  resetCreditsExpireLocal?: string;
  currency?: string;
  balance?: string;
  usagePercent?: number;
  periodLabel?: string;
  periodCaption?: string;
  usageResetLocal?: string;
  monthlyPercent?: number;
  monthlyResetLocal?: string;
  includedPercent?: number;
  apiPercent?: number;
  onDemandPercent?: number;
  grokBotPercent?: number;
  grokBotResetLocal?: string;
  unified?: boolean;
  productUsage?: { label: string; percent: number }[];
  prepaidBalanceCents?: number;
  claudeFiveHourPercent?: number;
  claudeSevenDayPercent?: number;
  rollingPercent?: number;
  weeklyPercent?: number;
  rollingResetLocal?: string;
  weeklyResetLocal?: string;
  claudeFiveHourResetLocal?: string;
  claudeSevenDayResetLocal?: string;
  argonSevenDayPercent?: number;
  argonSevenDayResetLocal?: string;
};

type GaugeWindow = RingWindow & {
  reset?: string;
  unknown?: boolean;
  group?: string;
  segments?: { label: string; percent: number }[];
};

function formatPercent(value: number): string {
  const pct = Math.max(0, Math.min(100, value));
  return pct > 0 && pct < 1 ? pct.toFixed(1) : String(Math.round(pct));
}

function windowOf(label: string, percent?: number, reset?: string): GaugeWindow | undefined {
  if (typeof percent !== "number" || Number.isNaN(percent)) return undefined;
  return { label, percent, reset };
}

/** Keep a shared reset time on the first lane only so Total/Cursor do not repeat it. */
function withUniqueResets(windows: GaugeWindow[]): GaugeWindow[] {
  const seen = new Set<string>();
  return windows.map((win) => {
    if (!win.reset) return win;
    if (seen.has(win.reset)) return { label: win.label, percent: win.percent };
    seen.add(win.reset);
    return win;
  });
}

function shownPercent(win: GaugeWindow): string {
  return win.unknown ? "--" : formatPercent(win.percent);
}

function legendText(win: GaugeWindow, compact = false): string {
  const label = win.group ? `${win.group} ${win.label}` : win.label;
  if (compact) return `${label} ${shownPercent(win)}`;
  return win.reset
    ? `${label} ${shownPercent(win)} RESET ${win.reset}`
    : `${label} ${shownPercent(win)}`;
}

/** X Subscription tiers; xAI still reports legacy names for some switched seats. */
const X_TIERS = new Set(["plus", "premium", "super", "ultra"]);

function displayPlan(plan?: string, xPool = false): string | undefined {
  if (!plan?.trim()) return undefined;
  if (xPool && !X_TIERS.has(plan.trim().toLowerCase())) return undefined;
  if (plan.trim().toLowerCase() === "antigravity") {
    return undefined;
  }
  return plan;
}

function toneOfPeak(peak: number | null): "ok" | "warn" | "danger" | undefined {
  if (peak == null) return undefined;
  if (peak >= 88) return "danger";
  if (peak >= 70) return "warn";
  return "ok";
}

function windowsFor(kind: Kind, service: AnyService): {
  outer?: GaugeWindow;
  inner?: GaugeWindow;
  extra: GaugeWindow[];
  legend?: GaugeWindow[];
  /** Bar-meter rows when they differ from the ring legend. */
  bars?: GaugeWindow[];
} {
  if (kind === "deepseek") return { extra: [] };

  if (kind === "grok") {
    const period = windowOf(service.periodLabel ?? "PERIOD", service.usagePercent, service.usageResetLocal);
    const month = windowOf("MONTH", service.monthlyPercent, service.monthlyResetLocal);
    const products = (service.productUsage ?? [])
      .filter((lane) => lane.percent > 0)
      .map((lane) => windowOf(lane.label, lane.percent))
      .filter((win): win is GaugeWindow => Boolean(win));
    if (service.unified && period && products.length > 0) {
      // One shared X pool: a single bar whose fill is split by product.
      const pool: GaugeWindow = {
        ...period,
        segments: products.map(({ label, percent }) => ({ label, percent })),
      };
      // The products add up to the pool, so both the bar and the ring draw a
      // single meter split by product; the legend lists each product.
      return {
        outer: pool,
        extra: products,
        legend: [pool, ...products],
        bars: [pool],
      };
    }
    if (period && month) return { outer: period, inner: month, extra: [] };
    if (period || month) return { outer: period ?? month, extra: [] };
    if (service.usageResetLocal) {
      return {
        outer: {
          label: service.periodLabel ?? "7D",
          percent: 0,
          reset: service.usageResetLocal,
          unknown: true,
        },
        extra: [],
      };
    }
    return { extra: [] };
  }

  if (kind === "cursor") {
    const included = windowOf("TOTAL", service.includedPercent, service.usageResetLocal);
    const auto = windowOf("CURSOR", service.usagePercent, service.usageResetLocal);
    const api = windowOf("THIRD PARTY", service.apiPercent);
    const onDemand = windowOf("EXTRA", service.onDemandPercent);
    const grokBot = windowOf("GROK BOT", service.grokBotPercent, service.grokBotResetLocal);
    const lanes = withUniqueResets(
      [included, auto, api, onDemand, grokBot].filter((win): win is GaugeWindow => Boolean(win)),
    );
    const ranked = [...lanes].sort((left, right) => right.percent - left.percent);
    return {
      outer: ranked[0],
      inner: ranked[1],
      extra: ranked.slice(2),
      legend: lanes,
    };
  }

  if (kind === "opencodego") {
    const seven = windowOf("7D", service.weeklyPercent, service.weeklyResetLocal);
    const five = windowOf("5H", service.rollingPercent, service.rollingResetLocal);
    const month = windowOf("MONTH", service.monthlyPercent, service.monthlyResetLocal);
    const [outer, inner, ...rest] = [seven, five, month].filter(
      (win): win is GaugeWindow => Boolean(win),
    );
    return { outer, inner, extra: rest };
  }

  if (kind === "antigravity") {
    const claudeSeven = windowOf("7D", service.claudeSevenDayPercent, service.claudeSevenDayResetLocal);
    const claudeFive = windowOf("5H", service.claudeFiveHourPercent, service.claudeFiveHourResetLocal);
    const geminiSeven = windowOf("7D", service.sevenDayPercent, service.sevenDayResetLocal);
    const geminiFive = windowOf("5H", service.fiveHourPercent, service.fiveHourResetLocal);
    const argonSeven = windowOf("7D", service.argonSevenDayPercent, service.argonSevenDayResetLocal);
    if (claudeSeven || claudeFive || argonSeven) {
      // Quota groups: Gemini, Claude and GPT models, and (paid Pro) Argon.
      const tag = (group: string, wins: (GaugeWindow | undefined)[]): GaugeWindow[] =>
        wins.filter((win): win is GaugeWindow => Boolean(win)).map((win) => ({ ...win, group }));
      const lanes = [
        ...tag("GEMINI", [geminiSeven, geminiFive]),
        ...tag("CLAUDE · GPT", [claudeSeven, claudeFive]),
        ...tag("ARGON", [argonSeven]),
      ];
      // The ring draws the busiest group (7D outside, 5H inside) so the centre
      // peak is always one of its arcs; the legend lists every group.
      const groupPeak = (group: string) =>
        Math.max(-1, ...lanes.filter((win) => win.group === group && !win.unknown).map((win) => win.percent));
      const ringGroup = ["CLAUDE · GPT", "ARGON"].reduce(
        (best, group) => (groupPeak(group) > groupPeak(best) ? group : best),
        "GEMINI",
      );
      const ringed = lanes.filter((win) => win.group === ringGroup);
      const [outer, inner] = ringed.length > 0 ? ringed : lanes;
      return {
        outer,
        inner,
        extra: lanes.filter((win) => win !== outer && win !== inner),
        legend: lanes,
      };
    }
  }

  const five = windowOf("5H", service.fiveHourPercent, service.fiveHourResetLocal);
  const seven = windowOf("7D", service.sevenDayPercent, service.sevenDayResetLocal);
  const extra = windowOf("EXTRA", service.extraUsagePercent);
  if (kind === "claude" && extra && !five && !seven) {
    return { outer: extra, extra: [] };
  }
  if (seven && five) return { outer: seven, inner: five, extra: [] };
  return { outer: seven ?? five ?? extra, extra: [] };
}

function useIpMeter(
  artSkin: ArtSkin,
  slot: string | undefined,
): {
  wide: boolean;
  ipLayout: "stack" | "split";
  panelRef: RefObject<HTMLElement>;
} {
  const panelRef = useRef<HTMLElement>(null) as RefObject<HTMLElement>;
  const slotted = slot === "hero" || slot === "top-left" || slot === "top-right";
  const [wide, setWide] = useState(slotted);
  const [ipLayout, setIpLayout] = useState<"stack" | "split">(slotted ? "split" : "stack");

  useLayoutEffect(() => {
    if (artSkin !== "ip") return;
    const node = panelRef.current;
    if (!node) return;
    const apply = () => {
      const rect = node.getBoundingClientRect();
      const ratio = rect.width / Math.max(rect.height, 1);
      setWide(slotted || ratio >= 1.35);
      setIpLayout(ratio < 0.75 ? "stack" : "split");
    };
    apply();
    const observer = new ResizeObserver(apply);
    observer.observe(node);
    return () => observer.disconnect();
  }, [artSkin, slotted]);

  return {
    wide,
    ipLayout,
    panelRef,
  };
}

function ariaFor(
  kind: Kind,
  title: string,
  service: AnyService,
  outer?: GaugeWindow,
  inner?: GaugeWindow,
  extra: GaugeWindow[] = [],
): string {
  if (kind === "deepseek") {
    if (service.balance) {
      return `${title} balance: ${service.currency ?? ""} ${service.balance}`.replace(/\s+/g, " ").trim();
    }
    return `${title} balance unavailable`;
  }
  if (!outer && !inner) {
    return `${title} usage unavailable`;
  }
  const parts = [outer, inner, ...extra]
    .filter((win): win is GaugeWindow => Boolean(win))
    .map((win) =>
      win.unknown
        ? `${win.group ? `${win.group} ` : ""}${win.label} unknown${win.reset ? `, resets ${win.reset}` : ""}`
        : `${win.group ? `${win.group} ` : ""}${win.label} ${formatPercent(win.percent)}%${win.reset ? `, resets ${win.reset}` : ""}`,
    );
  return `${title} usage: ${parts.join(", ")}`;
}

export function ServicePanel({
  kind,
  title,
  code,
  service,
  planSize,
  slot,
  placement,
  artSkin = "ip",
  topBarCover,
}: {
  kind: Kind;
  title: string;
  code: string;
  service: AnyService;
  planSize?: PlanSize;
  slot?: string;
  placement?: GridSlot;
  artSkin?: ArtSkin;
  /** IP-skin clock/buttons floating over this card's header. */
  topBarCover?: "clock" | "controls" | "both";
}) {
  const { wide, ipLayout, panelRef } = useIpMeter(artSkin, slot);
  const titleId = `panel-${kind}-title`;
  const percents = [
    service.fiveHourPercent,
    service.sevenDayPercent,
    service.extraUsagePercent,
    service.usagePercent,
    service.monthlyPercent,
    service.includedPercent,
    service.apiPercent,
    service.onDemandPercent,
    service.grokBotPercent,
    service.claudeFiveHourPercent,
    service.claudeSevenDayPercent,
    service.argonSevenDayPercent,
    service.rollingPercent,
    service.weeklyPercent,
  ];
  const mood = mascotMood(service.status, percents);
  const peak = peakPercent(percents);
  const portrait = artSkin === "ip" ? mascotSrc(kind, mood, artSkin) : "";
  const cornerPortrait = artSkin === "ip" ? cornerMascotSrc(kind, mood) : undefined;
  const { outer, inner, extra, legend: orderedLegend, bars } = windowsFor(kind, service);
  const legend =
    orderedLegend ??
    [outer, inner, ...extra].filter((win): win is GaugeWindow => Boolean(win));
  const meter: "ring" | "bar" = artSkin !== "ip" || wide ? "bar" : "ring";
  const resetCredits = service.resetCreditsAvailable;
  const ringOuter = outer?.unknown ? undefined : outer;
  const ringInner = inner?.unknown ? undefined : inner;
  const hasUsage = Boolean(outer || inner);
  const planLabel = displayPlan(service.plan, kind === "grok" && Boolean(service.unified));
  const showPlan =
    (kind === "codex" ||
      kind === "claude" ||
      kind === "grok" ||
      kind === "cursor" ||
      kind === "antigravity") &&
    Boolean(planLabel) &&
    !(artSkin === "ip" && planLabel?.trim().toUpperCase() === title.toUpperCase());
  const panelTone =
    artSkin !== "ip"
      ? undefined
      : mood === "error" || mood === "over"
        ? "danger"
        : mood === "tense"
          ? "warn"
          : toneOfPeak(peak) ?? "ok";
  const ariaLabel = ariaFor(kind, title, service, outer, inner, extra);

  let readout: ReactNode;
  if (kind === "deepseek") {
    readout = service.balance ? (
      <span className="balance-amount">
        <span className="balance-currency">{service.currency ?? ""}</span>
        <span className={`balance-number${service.balance.length > 12 ? " balance-number-long" : ""}`}>
          {service.balance}
        </span>
      </span>
    ) : (
      <span className="balance-number">--</span>
    );
  } else if (peak != null) {
    readout = (
      <>
        {formatPercent(peak)}
        <i>%</i>
      </>
    );
  } else {
    readout = (
      <>
        --
        <i>%</i>
      </>
    );
  }

  return (
    <section
      ref={panelRef}
      className={`panel panel-${kind}`}
      aria-labelledby={titleId}
      data-plan-size={planSize}
      data-slot={slot}
      data-meter={artSkin === "ip" ? meter : undefined}
      data-emerge={artSkin === "ip" ? IP_THEME[kind].emerge : undefined}
      data-ip-layout={artSkin === "ip" ? ipLayout : undefined}
      data-under={artSkin === "ip" ? topBarCover : undefined}
      data-tone={panelTone}
      style={
        {
          ...(placement
            ? {
                gridColumn: placement.column,
                gridRow: placement.row,
              }
            : {}),
          ...(artSkin === "ip" ? ipVars(kind) : {}),
        } satisfies CSSProperties
      }
    >
      <div className="panel-edge" aria-hidden />
      <CornerBrackets />

      <header className="panel-head">
        <div className="panel-titles">
          <div className="panel-title-row">
            <span className="panel-glyph" aria-hidden />
            <h2 className="panel-title" id={titleId}>
              {title}
            </h2>
            {artSkin === "ip" && showPlan && planLabel && (
              <span className="panel-plan">{planLabel.toUpperCase()}</span>
            )}
          </div>
          <span className="panel-sub">
            {kind === "deepseek" ? "ACCOUNT OVERVIEW" : "USAGE OVERVIEW"}
          </span>
        </div>
        <div className="panel-head-right">
          {artSkin !== "ip" && showPlan && planLabel && (
            <span className="panel-plan">{planLabel.toUpperCase()}</span>
          )}
          <span className="panel-code">{code}</span>
        </div>
      </header>

      <div className="panel-body">
        {artSkin === "ip" && (
          <div className="gauge-slot">
            {meter === "bar" ? (
              <div
                className="ip-portrait"
                data-mood={mood}
                data-corner-art={cornerPortrait ? "true" : undefined}
                role="img"
                aria-label={ariaLabel}
              >
                {(cornerPortrait ?? portrait) && (
                  <img
                    className="panel-mascot"
                    src={cornerPortrait ?? portrait}
                    alt=""
                    data-mascot={mood}
                    data-pack={artSkin}
                    draggable={false}
                  />
                )}
              </div>
            ) : (
              <RingGauge
                outer={kind === "deepseek" ? undefined : ringOuter}
                inner={kind === "deepseek" ? undefined : ringInner}
                readout={readout}
                mood={mood}
                ariaLabel={ariaLabel}
                artSkin={artSkin}
              >
                {portrait && (
                  <>
                    <img
                      className="panel-mascot"
                      src={portrait}
                      alt=""
                      data-mascot={mood}
                      data-pack={artSkin}
                      draggable={false}
                    />
                    <span className="mascot-halo" aria-hidden />
                  </>
                )}
              </RingGauge>
            )}
          </div>
        )}
        {meter === "bar" && (
          <UsageBars
            skin={artSkin === "ip" ? "ip" : "ring"}
            windows={kind === "deepseek" ? [] : (bars ?? legend)}
            mood={mood}
            ariaLabel={ariaLabel}
            balance={
              kind === "deepseek"
                ? { currency: service.currency, amount: service.balance }
                : undefined
            }
          />
        )}
        <div className="gauge-foot">
        <div
          className="gauge-legend"
          data-compact={legend.length >= 4 && legend.some((win) => win.group) ? "true" : undefined}
        >
          {kind === "deepseek" ? (
            <span className="gauge-legend-item" data-window="BALANCE">
              {artSkin === "ip" ? "BALANCE" : "CURRENT BALANCE"}
            </span>
          ) : hasUsage ? (
            legend.map((win) => (
              <span key={`${win.group ?? ""}:${win.label}`} className="gauge-legend-item" data-window={win.label}>
                {artSkin === "ip" ? (
                  <>
                    <span className="legend-main">
                      {win.group ? `${win.group} ` : ""}
                      {win.label} {shownPercent(win)}
                    </span>
                    {win.reset ? <span className="legend-reset">{win.reset}</span> : null}
                  </>
                ) : (
                  legendText(win)
                )}
              </span>
            ))
          ) : (
            <span className="usage-unavailable">USAGE DATA UNAVAILABLE</span>
          )}
        </div>
        {(kind === "codex" || kind === "grok") && typeof resetCredits === "number" && resetCredits > 0 && (
          <div
            className="reset-credits"
            role="status"
            aria-label={`${resetCredits} banked resets available${
              service.resetCreditsExpireLocal
                ? `, first expires ${service.resetCreditsExpireLocal}`
                : ""
            }`}
          >
            BANKED RESETS · {resetCredits} AVAILABLE
            {service.resetCreditsExpireLocal ? ` · FIRST EXP ${service.resetCreditsExpireLocal}` : ""}
          </div>
        )}
        </div>
      </div>

      <footer className="panel-status">
        <span className="ps-label">
          {kind === "deepseek" ? "ACCOUNT STATUS" : "MODEL STATUS"}
        </span>
        <span className="ps-rule" aria-hidden />
        {kind === "claude" && service.cooldownUntilLocal && (
          <span className="cooldown">COOLDOWN · {service.cooldownUntilLocal}</span>
        )}
        <StatusChip
          status={service.status}
          fromCache={service.fromCache}
          dataMayBeStale={service.dataMayBeStale}
        />
      </footer>
    </section>
  );
}
