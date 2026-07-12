// Decorative left rail — rotated label, a live tag, hex and tick marks.
export function SideRail() {
  return (
    <aside className="rail" aria-hidden>
      <span className="rail-rec">● LIVE</span>
      <span className="rail-text">AI · USAGE · MONITOR</span>
      <span className="rail-hex" />
      <span className="rail-ticks">
        {Array.from({ length: 9 }).map((_, i) => (
          <i key={i} />
        ))}
      </span>
      <span className="rail-code">MGI-09</span>
    </aside>
  );
}
