// Segmented usage meter. Lit segments use the panel accent color until the
// high end, where they escalate to amber (>=80%) / red (>=90%) per the brief.

const SEGMENTS = 30;

function segClass(posPct: number): string {
  if (posPct >= 90) return "seg-red";
  if (posPct >= 80) return "seg-amber";
  return "seg-on";
}

export function ProgressMeter({
  label,
  sub,
  percent,
  resetLabel,
}: {
  label: string;
  sub: string;
  percent?: number;
  resetLabel?: string;
}) {
  const has = typeof percent === "number" && !Number.isNaN(percent);
  const pct = has ? Math.max(0, Math.min(100, percent!)) : 0;
  const lit = Math.round((pct / 100) * SEGMENTS);

  const segs = Array.from({ length: SEGMENTS }, (_, i) => {
    const on = i < lit;
    const pos = ((i + 1) / SEGMENTS) * 100;
    return <span key={i} className={`seg ${on ? segClass(pos) : "seg-off"}`} />;
  });

  return (
    <div className="meter">
      <div className="meter-head">
        <span className="meter-label">{label}</span>
        <span className="meter-value">
          {has ? Math.round(pct) : "--"}
          <i>%</i>
        </span>
      </div>
      <div className="meter-track">{segs}</div>
      <div className="meter-foot">
        <span className="meter-sub">{sub}</span>
        <span className="meter-reset">{resetLabel ? `RESET ${resetLabel}` : ""}</span>
      </div>
    </div>
  );
}
