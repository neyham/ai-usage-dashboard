// Bottom telemetry strip — mostly ambient, but carries the real "last refresh"
// label and the manual refresh control.
export function TelemetryBar({
  refreshedLabel,
  onRefresh,
}: {
  refreshedLabel: string;
  onRefresh: () => void;
}) {
  return (
    <footer className="telemetry">
      <span className="tm-item">
        <i className="tm-dot on" /> NET OK
      </span>
      <span className="tm-item">LINK ▮▮▮▮▮▯▯</span>
      <span className="tm-item">SCAN 0x1F</span>
      <span className="tm-hex" aria-hidden />
      <span className="tm-grow" />
      <span className="tm-item tm-mark">AI USAGE · LAST SYNC {refreshedLabel}</span>
      <button className="tm-refresh" onClick={onRefresh}>
        REFRESH [F5]
      </button>
    </footer>
  );
}
