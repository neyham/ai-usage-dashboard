// Per-panel status: ONLINE / CACHED / STALE, with the Chinese status text.

export function StatusChip({
  status,
  fromCache,
  dataMayBeStale,
}: {
  status: string;
  fromCache: boolean;
  dataMayBeStale: boolean;
}) {
  let cls = "chip-ok";
  let tag = "ONLINE";
  const authIssue = /AUTH|LOGIN|REFRESH/.test(status.toUpperCase());
  if (authIssue) {
    cls = "chip-auth";
    tag = "AUTH";
  } else if (dataMayBeStale) {
    cls = "chip-stale";
    tag = "STALE";
  } else if (fromCache) {
    cls = "chip-cache";
    tag = "CACHED";
  }

  return (
    <div className={`chip ${cls}`}>
      <span className="chip-dot" />
      <span className="chip-tag">{tag}</span>
      <span className="chip-text">{status}</span>
    </div>
  );
}
