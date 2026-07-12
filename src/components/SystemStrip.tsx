import type { SummaryStatus } from "../types";

// Centered alert banner. Color + text are driven by the real summary status,
// so the "WARNING" read-out actually means something.
const MAP: Record<SummaryStatus, [string, string]> = {
  ok: ["sys-nominal", "● SYSTEM NOMINAL"],
  refreshing: ["sys-sync", "◴ SYNCHRONIZING"],
  partial: ["sys-warn", "▲ WARNING · SERVICE DEGRADED"],
  error: ["sys-alert", "■ ALERT · SERVICE FAILURE"],
  idle: ["sys-sync", "◴ STANDBY"],
};

export function SystemStrip({
  status,
  message,
}: {
  status: SummaryStatus;
  message?: string | null;
}) {
  const [cls, defaultText] = MAP[status] ?? MAP.idle;
  const text = message ? `■ ALERT · ${message}` : defaultText;
  return (
    <div className={`sysstrip ${cls}`} role="status" aria-live="polite" aria-atomic="true">
      <span className="sys-hatch" aria-hidden />
      <span className="sys-text">{text}</span>
      <span className="sys-hatch" aria-hidden />
    </div>
  );
}
