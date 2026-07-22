import type { ClaudeService, CodexService, DeepSeekService } from "../types";
import { ProgressMeter } from "./ProgressMeter";
import { StatusChip } from "./StatusChip";
import { CornerBrackets } from "./CornerBrackets";
import { Reticle } from "./Reticle";

type Kind = "codex" | "claude" | "deepseek";

type AnyService = (CodexService | ClaudeService | DeepSeekService) & {
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
};

// Static ambient histogram for the balance panel (decorative).
const HISTO = [42, 64, 50, 72, 56, 84, 60, 76, 48, 68, 58, 90, 46, 70, 62, 80];

export function ServicePanel({
  kind,
  title,
  code,
  service,
}: {
  kind: Kind;
  title: string;
  code: string;
  service: AnyService;
}) {
  const titleId = `panel-${kind}-title`;
  const hasFiveHour = typeof service.fiveHourPercent === "number";
  const hasSevenDay = typeof service.sevenDayPercent === "number";
  const showClaudeExtra =
    kind === "claude" &&
    !hasFiveHour &&
    !hasSevenDay &&
    typeof service.extraUsagePercent === "number";
  const hasUsage = hasFiveHour || hasSevenDay || showClaudeExtra;
  const resetCredits = service.resetCreditsAvailable;

  return (
    <section className={`panel panel-${kind}`} aria-labelledby={titleId}>
      <div className="panel-edge" aria-hidden />
      <CornerBrackets />
      <Reticle />

      <header className="panel-head">
        <div className="panel-titles">
          <div className="panel-title-row">
            <span className="panel-glyph" aria-hidden />
            <h2 className="panel-title" id={titleId}>
              {title}
            </h2>
          </div>
          <span className="panel-sub">
            {kind === "deepseek" ? "ACCOUNT OVERVIEW" : "USAGE OVERVIEW"}
          </span>
        </div>
        <div className="panel-head-right">
          {kind === "codex" && service.plan && (
            <span className="panel-plan">{service.plan.toUpperCase()}</span>
          )}
          <span className="panel-code">{code}</span>
        </div>
      </header>

      <div className="panel-body">
        {kind === "deepseek" ? (
          <div className="balance">
            <div className="balance-amount">
              {service.balance ? (
                <>
                  <span className="balance-currency">{service.currency ?? ""}</span>
                  <span className="balance-number">{service.balance}</span>
                </>
              ) : (
                <span className="balance-number">--</span>
              )}
            </div>
            <div className="balance-caption">CURRENT BALANCE</div>
            <div className="histo" aria-hidden>
              {HISTO.map((h, i) => (
                <span key={i} style={{ height: `${h}%` }} />
              ))}
            </div>
          </div>
        ) : (
          <>
            {hasFiveHour && (
              <ProgressMeter
                label="5H"
                sub="SESSION WINDOW"
                percent={service.fiveHourPercent}
                resetLabel={service.fiveHourResetLocal}
              />
            )}
            {hasSevenDay && (
              <ProgressMeter
                label="7D"
                sub="WEEKLY WINDOW"
                percent={service.sevenDayPercent}
                resetLabel={service.sevenDayResetLocal}
              />
            )}
            {showClaudeExtra && (
              <ProgressMeter
                label="EXTRA"
                sub="MONTHLY SPEND"
                percent={service.extraUsagePercent}
              />
            )}
            {!hasUsage && <div className="usage-unavailable">USAGE DATA UNAVAILABLE</div>}
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
                <span className="reset-credits-title">BANKED RESETS</span>
                <span className="reset-credits-meta">
                  <strong>{resetCredits} AVAILABLE</strong>
                  {service.resetCreditsExpireLocal && (
                    <small>FIRST EXP {service.resetCreditsExpireLocal}</small>
                  )}
                </span>
              </div>
            )}
          </>
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
