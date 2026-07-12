import { useEffect, useState } from "react";

// The oversized clock cluster (top-right of the bar).
export function ClockHeader() {
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(id);
  }, []);

  const time = now.toLocaleTimeString("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
  const date = now
    .toLocaleDateString("en-CA", { year: "numeric", month: "2-digit", day: "2-digit" })
    .replace(/-/g, "·");
  const weekday = now.toLocaleDateString("en-US", { weekday: "short" }).toUpperCase();

  return (
    <div className="clockwrap">
      <time className="clock" dateTime={now.toISOString()}>
        {time}
      </time>
      <div className="clock-date">
        {date} <span className="clock-day">{weekday}</span>
      </div>
    </div>
  );
}
