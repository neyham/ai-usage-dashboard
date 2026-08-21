import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { ArtSkin } from "../ipTheme";
import type { MascotMood } from "../mascot";

export type RingWindow = { percent: number; label: string };

const CX = 100;
const CY = 100;
const OUTER_R = 90;
const INNER_R = 76;
const OUTER_WIDTH = 10;
const INNER_WIDTH = 7;
const ARC_FRACTION = 0.75;
const START_ROTATE = 135;

function circumference(radius: number): number {
  return 2 * Math.PI * radius;
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, value));
}

function toneOf(percent: number | undefined): "ok" | "warn" | "danger" | "idle" {
  if (percent == null) return "idle";
  if (percent >= 88) return "danger";
  if (percent >= 70) return "warn";
  return "ok";
}

function peakOf(outer?: RingWindow, inner?: RingWindow): number | undefined {
  const values = [outer?.percent, inner?.percent].filter(
    (value): value is number => typeof value === "number" && Number.isFinite(value),
  );
  if (values.length === 0) return undefined;
  return Math.max(...values);
}

function polar(radius: number, mathDeg: number): { x: number; y: number } {
  const rad = (mathDeg * Math.PI) / 180;
  return { x: CX + radius * Math.cos(rad), y: CY - radius * Math.sin(rad) };
}

function RingArc({
  radius,
  trackWidth,
  fillWidth,
  percent,
  ready,
  showCap,
}: {
  radius: number;
  trackWidth: number;
  fillWidth: number;
  percent: number;
  ready: boolean;
  showCap: boolean;
}) {
  const circ = circumference(radius);
  const arc = circ * ARC_FRACTION;
  const used = (clampPercent(percent) / 100) * arc;
  const end = polar(radius, 225 - 2.7 * clampPercent(percent));
  const tone = toneOf(percent);
  return (
    <g className={`gauge-arc gauge-arc-${tone}`}>
      <circle
        className="gauge-track"
        cx={CX}
        cy={CY}
        r={radius}
        fill="none"
        strokeWidth={trackWidth}
        strokeLinecap="butt"
        strokeDasharray={`${arc} ${circ}`}
        transform={`rotate(${START_ROTATE} ${CX} ${CY})`}
      />
      <circle
        className="gauge-fill"
        cx={CX}
        cy={CY}
        r={radius}
        fill="none"
        stroke="currentColor"
        strokeWidth={fillWidth}
        strokeLinecap="butt"
        strokeDasharray={`${ready ? used : 0} ${circ}`}
        transform={`rotate(${START_ROTATE} ${CX} ${CY})`}
      />
      {showCap && percent > 0 && (
        <circle className="gauge-cap" cx={end.x} cy={end.y} r={3} />
      )}
    </g>
  );
}

export function RingGauge({
  outer,
  inner,
  readout,
  mood,
  children,
  ariaLabel,
  artSkin = "ip",
}: {
  outer?: RingWindow;
  inner?: RingWindow;
  readout: ReactNode;
  mood: MascotMood;
  children?: ReactNode;
  ariaLabel: string;
  artSkin?: ArtSkin;
}) {
  const stageRef = useRef<HTMLDivElement>(null);
  const [diameter, setDiameter] = useState(200);
  const [ready, setReady] = useState(false);

  useLayoutEffect(() => {
    const node = stageRef.current;
    if (!node) return;
    const apply = () => {
      const rect = node.getBoundingClientRect();
      setDiameter(Math.min(rect.width, rect.height));
    };
    apply();
    const observer = new ResizeObserver(apply);
    observer.observe(node);
    const frame = requestAnimationFrame(() => setReady(true));
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
  }, []);

  const ip = artSkin === "ip";
  const compact = diameter < 120;
  const hideTicks = ip || diameter < 96;
  const outerR = ip ? 93 : OUTER_R;
  const innerR = ip ? 78 : INNER_R;
  const outerTrack = ip ? (compact ? 14 : 16) : OUTER_WIDTH;
  const outerFill = ip ? (compact ? 8 : 10) : OUTER_WIDTH;
  const innerTrack = ip ? (compact ? 10 : 12) : INNER_WIDTH;
  const innerFill = ip ? 8 : INNER_WIDTH;
  const shownInner = compact ? undefined : inner;
  const shownOuter = compact
    ? outer || inner
      ? { percent: peakOf(outer, inner) ?? 0, label: outer?.label ?? inner?.label ?? "" }
      : undefined
    : outer;
  const rings = shownInner ? 2 : 1;
  const tone = toneOf(peakOf(shownOuter, shownInner));
  const ticks = [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100];

  return (
    <div
      ref={stageRef}
      className="gauge-stage"
      data-mood={mood}
      data-tone={tone}
      data-rings={String(rings)}
      data-ticks={hideTicks ? "0" : "1"}
      data-art={artSkin}
      data-empty={shownOuter ? undefined : "true"}
      role="img"
      aria-label={ariaLabel}
    >
      <svg className="gauge-svg" viewBox="0 0 200 200" aria-hidden>
        {!hideTicks &&
          ticks.map((percent) => {
            const a = polar(96, 225 - 2.7 * percent);
            const b = polar(101, 225 - 2.7 * percent);
            return (
              <line
                key={percent}
                className="gauge-tick"
                x1={a.x}
                y1={a.y}
                x2={b.x}
                y2={b.y}
              />
            );
          })}
        {shownOuter ? (
          <RingArc
            radius={outerR}
            trackWidth={outerTrack}
            fillWidth={outerFill}
            percent={shownOuter.percent}
            ready={ready}
            showCap={!ip}
          />
        ) : (
          <circle
            className="gauge-track gauge-track-empty"
            cx={CX}
            cy={CY}
            r={outerR}
            fill="none"
            strokeWidth={outerTrack}
            strokeLinecap="butt"
            strokeDasharray={`${circumference(outerR) * ARC_FRACTION} ${circumference(outerR)}`}
            transform={`rotate(${START_ROTATE} ${CX} ${CY})`}
          />
        )}
        {shownInner && (
          <RingArc
            radius={innerR}
            trackWidth={innerTrack}
            fillWidth={innerFill}
            percent={shownInner.percent}
            ready={ready}
            showCap={!ip}
          />
        )}
      </svg>
      {children && <div className="gauge-figure">{children}</div>}
      <div className={`gauge-readout${mood === "over" ? " is-pulse" : ""}`}>{readout}</div>
    </div>
  );
}
