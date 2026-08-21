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
} from "../types";
import type { GridSlot, PlanSize } from "../planLayout";
import { IP_THEME, ipVars, type ArtSkin } from "../ipTheme";
import { mascotMood, mascotSrc, peakPercent } from "../mascot";
import { RingGauge, type RingWindow } from "./RingGauge";
import { StatusChip } from "./StatusChip";
import { CornerBrackets } from "./CornerBrackets";
import { UsageBars } from "./UsageBars";

type Kind = "codex" | "claude" | "deepseek" | "grok" | "cursor" | "antigravity";

type AnyService = (
  | CodexService
  | ClaudeService
  | DeepSeekService
  | GrokService
  | CursorService
  | AntigravityService
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
  apiPercent?: number;
  onDemandPercent?: number;
};

type GaugeWindow = RingWindow & { reset?: string };

function formatPercent(value: number): string {
  const pct = Math.max(0, Math.min(100, value));
  return pct > 0 && pct < 1 ? pct.toFixed(1) : String(Math.round(pct));
}

function windowOf(label: string, percent?: number, reset?: string): GaugeWindow | undefined {
  if (typeof percent !== "number" || Number.isNaN(percent)) return undefined;
  return { label, percent, reset };
}

function legendText(win: GaugeWindow, compact = false): string {
  if (compact) return `${win.label} ${formatPercent(win.percent)}`;
  return win.reset
    ? `${win.label} ${formatPercent(win.percent)} RESET ${win.reset}`
    : `${win.label} ${formatPercent(win.percent)}`;
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
} {
  if (kind === "deepseek") return { extra: [] };

  if (kind === "grok") {
    const period = windowOf(service.periodLabel ?? "PERIOD", service.usagePercent, service.usageResetLocal);
    const month = windowOf("MONTH", service.monthlyPercent, service.monthlyResetLocal);
    if (period && month) return { outer: period, inner: month, extra: [] };
    return { outer: period ?? month, extra: [] };
  }

  if (kind === "cursor") {
    const month = windowOf("MONTH", service.usagePercent, service.usageResetLocal);
    const api = windowOf("API", service.apiPercent);
    const onDemand = windowOf("EXTRA", service.onDemandPercent);
    if (month && api) return { outer: month, inner: api, extra: onDemand ? [onDemand] : [] };
    return { outer: month ?? api, extra: onDemand ? [onDemand] : [] };
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
  meter: "ring" | "bar";
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
    meter: artSkin === "ip" && wide ? "bar" : "ring",
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
    .map((win) => `${win.label} ${formatPercent(win.percent)}%${win.reset ? `, resets ${win.reset}` : ""}`);
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
}: {
  kind: Kind;
  title: string;
  code: string;
  service: AnyService;
  planSize?: PlanSize;
  slot?: string;
  placement?: GridSlot;
  artSkin?: ArtSkin;
}) {
  const { meter, ipLayout, panelRef } = useIpMeter(artSkin, slot);
  const titleId = `panel-${kind}-title`;
  const percents = [
    service.fiveHourPercent,
    service.sevenDayPercent,
    service.extraUsagePercent,
    service.usagePercent,
    service.monthlyPercent,
    service.apiPercent,
    service.onDemandPercent,
  ];
  const mood = mascotMood(service.status, percents);
  const peak = peakPercent(percents);
  const portrait = mascotSrc(kind, mood, artSkin);
  const { outer, inner, extra } = windowsFor(kind, service);
  const legend = [outer, inner, ...extra].filter((win): win is GaugeWindow => Boolean(win));
  const resetCredits = service.resetCreditsAvailable;
  const hasUsage = Boolean(outer || inner);
  const showPlan =
    (kind === "codex" || kind === "grok" || kind === "cursor" || kind === "antigravity") &&
    Boolean(service.plan) &&
    !(artSkin === "ip" && service.plan?.trim().toUpperCase() === title.toUpperCase());
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
            {artSkin === "ip" && showPlan && service.plan && (
              <span className="panel-plan">{service.plan.toUpperCase()}</span>
            )}
          </div>
          <span className="panel-sub">
            {kind === "deepseek" ? "ACCOUNT OVERVIEW" : "USAGE OVERVIEW"}
          </span>
        </div>
        <div className="panel-head-right">
          {artSkin !== "ip" && showPlan && service.plan && (
            <span className="panel-plan">{service.plan.toUpperCase()}</span>
          )}
          <span className="panel-code">{code}</span>
        </div>
      </header>

      <div className="panel-body">
        <div className="gauge-slot">
          {meter === "bar" ? (
            <div className="ip-portrait" data-mood={mood} role="img" aria-label={ariaLabel}>
              {portrait && (
                <img
                  className="panel-mascot"
                  src={portrait}
                  alt=""
                  data-mascot={mood}
                  data-pack={artSkin}
                  draggable={false}
                />
              )}
            </div>
          ) : (
            <RingGauge
              outer={kind === "deepseek" ? undefined : outer}
              inner={kind === "deepseek" ? undefined : inner}
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
        {meter === "bar" && (
          <UsageBars
            windows={kind === "deepseek" ? [] : legend}
            mood={mood}
            balance={
              kind === "deepseek"
                ? { currency: service.currency, amount: service.balance }
                : undefined
            }
          />
        )}
        <div className="gauge-legend">
          {kind === "deepseek" ? (
            <span className="gauge-legend-item" data-window="BALANCE">
              {artSkin === "ip" ? "BALANCE" : "CURRENT BALANCE"}
            </span>
          ) : hasUsage ? (
            legend.map((win) => (
              <span key={win.label} className="gauge-legend-item" data-window={win.label}>
                {artSkin === "ip" ? (
                  <>
                    <span className="legend-main">
                      {win.label} {formatPercent(win.percent)}
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
        {kind === "codex" && typeof resetCredits === "number" && resetCredits > 0 && (
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
