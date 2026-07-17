import { RefreshCw, Settings } from "lucide-react";
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
  judgeDemo,
  onRefresh,
  onSettings,
}: {
  refreshedLabel: string;
  status: SummaryStatus;
  errorMessage?: string | null;
  judgeDemo?: boolean;
  onRefresh: () => void;
  onSettings?: () => void;
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
      {judgeDemo && <span className="tm-demo">SYNTHETIC DEMO · OFFLINE</span>}
      {errorMessage && <span className="tm-error">{errorMessage}</span>}
      <span className="tm-actions">
        {onSettings && (
          <button
            type="button"
            className="icon-button tm-settings"
            onClick={onSettings}
            aria-label="Provider settings"
            title="Provider settings"
          >
            <Settings size={19} aria-hidden />
          </button>
        )}
        <button
          type="button"
          className="tm-refresh"
          onClick={onRefresh}
          disabled={refreshing}
          aria-busy={refreshing}
        >
          <RefreshCw className={refreshing ? "tm-refresh-icon is-spinning" : "tm-refresh-icon"} size={16} aria-hidden />
          {refreshing ? "SYNCING" : "REFRESH"}
        </button>
      </span>
    </footer>
  );
}
