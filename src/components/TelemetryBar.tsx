import type { SummaryStatus } from "../types";

// Bottom telemetry strip — mostly ambient, but carries the real "last refresh"
// label and the manual refresh control.
const NET_STATE: Record<SummaryStatus, [string, string]> = {
  ok: ["on", "DATA OK"],
  refreshing: ["sync", "DATA SYNC"],
  partial: ["warn", "DATA DEGRADED"],
  error: ["alert", "LIVE DATA UNAVAILABLE"],
  idle: ["idle", "DATA STANDBY"],
};

export function TelemetryBar({
  refreshedLabel,
  status,
  errorMessage,
  onRefresh,
}: {
  refreshedLabel: string;
  status: SummaryStatus;
  errorMessage?: string | null;
  onRefresh: () => void;
}) {
  const [netClass, netLabel] = NET_STATE[status] ?? NET_STATE.idle;
  const refreshing = status === "refreshing";

  return (
    <footer className="telemetry">
      <span className={`tm-item tm-net tm-net-${netClass}`}>
        <i className="tm-dot" aria-hidden /> {netLabel}
      </span>
      <span className="tm-item tm-ambient" aria-hidden>
        LINK ▮▮▮▮▮▯▯
      </span>
      <span className="tm-item tm-ambient" aria-hidden>
        SCAN 0x1F
      </span>
      <span className="tm-hex tm-ambient" aria-hidden />
      <span className="tm-grow" aria-hidden />
      <span className="tm-item tm-mark">AI USAGE · LAST SYNC {refreshedLabel}</span>
      {errorMessage && <span className="tm-error">{errorMessage}</span>}
      <button
        type="button"
        className="tm-refresh"
        onClick={onRefresh}
        disabled={refreshing}
        aria-busy={refreshing}
      >
        {refreshing ? "SYNCING" : "REFRESH [F5]"}
      </button>
    </footer>
  );
}
