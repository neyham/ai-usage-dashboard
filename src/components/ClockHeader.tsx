import { useEffect, useState } from "react";

// The oversized clock cluster (top-right of the bar).
export function ClockHeader({ lowPowerMode }: { lowPowerMode: boolean }) {
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    let timer: number | undefined;
    const cadence = lowPowerMode ? 60_000 : 1_000;

    const schedule = () => {
      window.clearTimeout(timer);
      if (document.hidden) return;
      const delay = cadence - (Date.now() % cadence) + 20;
      timer = window.setTimeout(() => {
        setNow(new Date());
        schedule();
      }, delay);
    };
    const onVisibilityChange = () => {
      if (!document.hidden) setNow(new Date());
      schedule();
    };

    setNow(new Date());
    schedule();
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, [lowPowerMode]);

  const time = now.toLocaleTimeString("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    ...(lowPowerMode ? {} : { second: "2-digit" as const }),
    hour12: false,
  });
  const date = now
    .toLocaleDateString("en-CA", { year: "numeric", month: "2-digit", day: "2-digit" })
    .replace(/-/g, "·");
  const weekday = now.toLocaleDateString("en-US", { weekday: "short" }).toUpperCase();

  return (
    <div className="clockwrap">
      <time
        className="clock"
        dateTime={now.toISOString()}
        data-cadence={lowPowerMode ? "minute" : "second"}
      >
        {time}
      </time>
      <div className="clock-date">
        {date} <span className="clock-day">{weekday}</span>
      </div>
    </div>
  );
}
