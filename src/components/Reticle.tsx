// Decorative targeting radar in each panel — pure ambience, no data.
export function Reticle() {
  return (
    <svg className="reticle" viewBox="0 0 100 100" aria-hidden>
      <circle cx="50" cy="50" r="46" className="ret-ring" />
      <circle cx="50" cy="50" r="33" className="ret-ring ret-dim" />
      <circle cx="50" cy="50" r="20" className="ret-ring ret-dim" />
      <circle cx="50" cy="50" r="3" className="ret-dot" />
      <line x1="50" y1="1" x2="50" y2="13" className="ret-tick" />
      <line x1="50" y1="87" x2="50" y2="99" className="ret-tick" />
      <line x1="1" y1="50" x2="13" y2="50" className="ret-tick" />
      <line x1="87" y1="50" x2="99" y2="50" className="ret-tick" />
      <g className="ret-spin">
        <path d="M50 4 A46 46 0 0 1 96 50" className="ret-sweep" />
        <circle cx="50" cy="4" r="1.8" className="ret-blip" />
      </g>
    </svg>
  );
}
