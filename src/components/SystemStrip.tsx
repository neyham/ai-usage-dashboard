import type { SummaryStatus } from "../types";

// Centered alert banner. Color + text are driven by the real summary status,
// so the "WARNING" read-out actually means something.
const MAP: Record<SummaryStatus, [string, string]> = {
  ok: ["sys-nominal", "● SYSTEM NOMINAL"],
  refreshing: ["sys-sync", "◴ SYNCHRONIZING"],
  partial: ["sys-warn", "▲ WARNING · CACHED DATA"],
  error: ["sys-alert", "■ ALERT · OFFLINE · SHOWING CACHE"],
  idle: ["sys-sync", "◴ STANDBY"],
};

export function SystemStrip({ status }: { status: SummaryStatus }) {
  const [cls, text] = MAP[status] ?? MAP.idle;
  return (
    <div className={`sysstrip ${cls}`}>
      <span className="sys-hatch" aria-hidden />
      <span className="sys-text">{text}</span>
      <span className="sys-hatch" aria-hidden />
    </div>
  );
}
