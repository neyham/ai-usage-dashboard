// Per-panel status derived from the backend status and cache freshness flags.

export function StatusChip({
  status,
  fromCache,
  dataMayBeStale,
}: {
  status: string;
  fromCache: boolean;
  dataMayBeStale: boolean;
}) {
  const normalized = status.trim().toUpperCase();
  let cls = "chip-error";
  let tag = "ERROR";
  const authIssue = /AUTH|LOGIN|REFRESH/.test(normalized);
  const explicitError = /ERROR|FAILED|OFFLINE|UNAVAILABLE/.test(normalized);
  if (authIssue) {
    cls = "chip-auth";
    tag = "AUTH";
  } else if (explicitError && !fromCache) {
    cls = "chip-error";
    tag = "ERROR";
  } else if (dataMayBeStale) {
    cls = "chip-stale";
    tag = "STALE";
  } else if (/RATE|LIMIT|COOLDOWN/.test(normalized)) {
    cls = "chip-warn";
    tag = "LIMIT";
  } else if (fromCache) {
    cls = "chip-cache";
    tag = "CACHED";
  } else if (normalized === "NOMINAL") {
    cls = "chip-ok";
    tag = "ONLINE";
  } else if (/AWAITING|IDLE|STANDBY/.test(normalized)) {
    cls = "chip-idle";
    tag = "WAIT";
  }

  return (
    <div className={`chip ${cls}`} role="status" aria-label={`${tag}: ${status}`}>
      <span className="chip-dot" aria-hidden />
      <span className="chip-tag">{tag}</span>
      <span className="chip-text">{status}</span>
    </div>
  );
}
