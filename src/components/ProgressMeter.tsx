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
  const displayPct = pct > 0 && pct < 1 ? pct.toFixed(1) : Math.round(pct).toString();
  const valueText = has
    ? `${displayPct}% used${resetLabel ? `, resets ${resetLabel}` : ""}`
    : "Usage unavailable";

  const segs = Array.from({ length: SEGMENTS }, (_, i) => {
    const on = i < lit;
    const pos = ((i + 1) / SEGMENTS) * 100;
    return <span key={i} className={`seg ${on ? segClass(pos) : "seg-off"}`} />;
  });

  return (
    <div
      className="meter"
      role="progressbar"
      aria-label={`${label} ${sub}`}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={has ? pct : undefined}
      aria-valuetext={valueText}
    >
      <div className="meter-head">
        <span className="meter-label">{label}</span>
        <span className="meter-value">
          {has ? displayPct : "--"}
          <i>%</i>
        </span>
      </div>
      <div className="meter-track" aria-hidden>
        {segs}
      </div>
      <div className="meter-foot">
        <span className="meter-sub">{sub}</span>
        <span className="meter-reset">{resetLabel ? `RESET ${resetLabel}` : ""}</span>
      </div>
    </div>
  );
}
