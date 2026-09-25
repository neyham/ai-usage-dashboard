import type { MascotMood } from "../mascot";
import type { RingWindow } from "./RingGauge";

type BarWindow = RingWindow & { reset?: string; unknown?: boolean };

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, value));
}

function formatPercent(value: number): string {
  const pct = clampPercent(value);
  return pct > 0 && pct < 1 ? pct.toFixed(1) : String(Math.round(pct));
}

function toneOf(percent: number): "ok" | "warn" | "danger" {
  if (percent >= 88) return "danger";
  if (percent >= 70) return "warn";
  return "ok";
}

export function UsageBars({
  windows,
  mood,
  balance,
  skin = "ip",
  ariaLabel,
  featuredLabel,
}: {
  windows: BarWindow[];
  mood: MascotMood;
  balance?: { currency?: string; amount?: string };
  skin?: "ip" | "ring";
  ariaLabel?: string;
  featuredLabel?: string;
}) {
  if (balance) {
    return (
      <div className="ip-bars" data-kind="balance" data-skin={skin}>
        <div className={`ip-bars-peak${mood === "over" ? " is-pulse" : ""}`}>
          <span className="balance-amount">
            <span className="balance-currency">{balance.currency ?? ""}</span>
            <span
              className={`balance-number${(balance.amount ?? "").length > 12 ? " balance-number-long" : ""}`}
            >
              {balance.amount ?? "--"}
            </span>
          </span>
        </div>
      </div>
    );
  }

  if (windows.length === 0) {
    return (
      <div className="ip-bars" data-empty="true" data-skin={skin}>
        <span className="usage-unavailable">USAGE DATA UNAVAILABLE</span>
      </div>
    );
  }

  const known = windows.filter((win) => !win.unknown);
  const featured = featuredLabel
    ? known.find((win) => win.label === featuredLabel)
    : undefined;
  const peak = featured?.percent ?? (known.length > 0 ? Math.max(...known.map((win) => win.percent)) : null);
  const peakTone = peak == null ? undefined : toneOf(peak);
  return (
    <div
      className="ip-bars"
      role="group"
      data-tone={peakTone}
      data-skin={skin}
      data-count={String(windows.length)}
      aria-label={ariaLabel}
    >
      {skin === "ip" && (
        <div
          className={`ip-bars-peak${mood === "over" ? " is-pulse" : ""}`}
          data-tone={peakTone}
        >
          {peak == null ? "--" : formatPercent(peak)}
          <i>%</i>
        </div>
      )}
      {windows.map((win) => (
        <div key={win.label} className="ip-bar" data-window={win.label}>
          <div className="ip-bar-meta">
            <span className="ip-bar-label">{win.label}</span>
            <span className="ip-bar-percent">
              {win.unknown ? "--" : formatPercent(win.percent)}
              <i>%</i>
            </span>
          </div>
          <div className="ip-bar-track">
            <i
              className="ip-bar-fill"
              style={{ width: `${win.unknown ? 0 : clampPercent(win.percent)}%` }}
            />
          </div>
          <span className="ip-bar-reset">{win.reset ? `RESET ${win.reset}` : "\u00a0"}</span>
        </div>
      ))}
    </div>
  );
}
