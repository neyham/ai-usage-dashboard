import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { LaunchMode, SummaryStatus, UsageSummary } from "./types";
import { ClockHeader } from "./components/ClockHeader";
import { ServicePanel } from "./components/ServicePanel";
import { SystemStrip } from "./components/SystemStrip";
import { SideRail } from "./components/SideRail";
import { TelemetryBar } from "./components/TelemetryBar";

const EMPTY: UsageSummary = {
  refreshedAt: null,
  status: "idle",
  services: {
    codex: { status: "AWAITING DATA", fromCache: false, dataMayBeStale: false },
    claude: { status: "AWAITING DATA", fromCache: false, dataMayBeStale: false },
    deepseek: { status: "AWAITING DATA", fromCache: false, dataMayBeStale: false },
  },
};

const REFRESH_TIMEOUT_MS = 180_000;
const LINK_LABEL: Record<SummaryStatus, string> = {
  idle: "LINK STANDBY",
  ok: "MAGI-LINK ACTIVE",
  refreshing: "LINK SYNCHRONIZING",
  partial: "LINK DEGRADED",
  error: "LINK UNAVAILABLE",
};

function isLaunchMode(value: string): value is LaunchMode {
  return value === "normal" || value === "fullscreen" || value === "screensaver";
}

export default function App() {
  const [summary, setSummary] = useState<UsageSummary>(EMPTY);
  const [launchMode, setLaunchMode] = useState<LaunchMode>("normal");
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [uiError, setUiError] = useState<string | null>(null);
  const refreshPendingRef = useRef(false);
  const refreshTimerRef = useRef<number | null>(null);

  const finishRefresh = useCallback(() => {
    refreshPendingRef.current = false;
    setIsRefreshing(false);
    if (refreshTimerRef.current !== null) {
      window.clearTimeout(refreshTimerRef.current);
      refreshTimerRef.current = null;
    }
  }, []);

  const refresh = useCallback(async () => {
    if (refreshPendingRef.current) return;

    refreshPendingRef.current = true;
    setIsRefreshing(true);
    setUiError(null);
    refreshTimerRef.current = window.setTimeout(() => {
      refreshPendingRef.current = false;
      refreshTimerRef.current = null;
      setIsRefreshing(false);
      setUiError("REFRESH TIMED OUT");
    }, REFRESH_TIMEOUT_MS);

    try {
      const started = await invoke<boolean>("refresh_now");
      if (!started) finishRefresh();
    } catch (err) {
      console.error("refresh_now failed", err);
      finishRefresh();
      setUiError("REFRESH COMMAND FAILED");
    }
  }, [finishRefresh]);

  useEffect(() => {
    let disposed = false;
    let receivedLiveSummary = false;
    let unlisten: (() => void) | undefined;

    const initialize = async () => {
      try {
        const stopListening = await listen<UsageSummary>("summary", (event) => {
          if (disposed) return;
          receivedLiveSummary = true;
          setSummary(event.payload);
          setUiError(null);
          if (event.payload.status !== "refreshing") finishRefresh();
        });
        if (disposed) {
          stopListening();
          return;
        }
        unlisten = stopListening;
      } catch (err) {
        console.error("summary listener failed", err);
        if (!disposed) setUiError("LIVE UPDATE CHANNEL OFFLINE");
      }

      try {
        const initialSummary = await invoke<UsageSummary>("get_summary");
        if (!disposed && !receivedLiveSummary) setSummary(initialSummary);
      } catch (err) {
        console.error("get_summary failed", err);
        if (!disposed && !receivedLiveSummary) setUiError("INITIAL DATA UNAVAILABLE");
      }
    };

    void initialize();

    return () => {
      disposed = true;
      unlisten?.();
      refreshPendingRef.current = false;
      if (refreshTimerRef.current !== null) {
        window.clearTimeout(refreshTimerRef.current);
        refreshTimerRef.current = null;
      }
    };
  }, [finishRefresh]);

  useEffect(() => {
    let disposed = false;
    invoke<string>("launch_mode")
      .then((mode) => {
        if (!disposed && isLaunchMode(mode)) setLaunchMode(mode);
      })
      .catch((err) => console.error("launch_mode failed", err));
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && launchMode !== "normal") {
        event.preventDefault();
        void invoke("exit_app").catch(() => {});
        return;
      }
      if (event.key === "F5" && launchMode !== "screensaver") {
        event.preventDefault();
        if (!event.repeat) void refresh();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [launchMode, refresh]);

  // Screensaver mode: exit on any real input after a short arming delay.
  useEffect(() => {
    if (launchMode !== "screensaver") return;

    let armed = false;
    let quitting = false;
    let origin: { x: number; y: number } | null = null;
    const armTimer = window.setTimeout(() => (armed = true), 1200);
    const quit = () => {
      if (!armed || quitting) return;
      quitting = true;
      void invoke("exit_app").catch(() => {
        quitting = false;
      });
    };
    const onPointerMove = (event: PointerEvent) => {
      if (!origin) {
        origin = { x: event.screenX, y: event.screenY };
        return;
      }
      const distance =
        Math.abs(event.screenX - origin.x) + Math.abs(event.screenY - origin.y);
      if (distance > 8) quit();
    };
    const onAnyInput = (event: Event) => {
      if (!event.defaultPrevented) quit();
    };

    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerdown", onAnyInput);
    window.addEventListener("touchstart", onAnyInput, { passive: true });
    window.addEventListener("keydown", onAnyInput);
    window.addEventListener("wheel", onAnyInput, { passive: true });

    return () => {
      window.clearTimeout(armTimer);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerdown", onAnyInput);
      window.removeEventListener("touchstart", onAnyInput);
      window.removeEventListener("keydown", onAnyInput);
      window.removeEventListener("wheel", onAnyInput);
    };
  }, [launchMode]);

  const refreshedLabel = summary.refreshedAt
    ? new Date(summary.refreshedAt).toLocaleTimeString("en-GB", {
        hour: "2-digit",
        minute: "2-digit",
      })
    : "--:--";

  const displayStatus: SummaryStatus = isRefreshing
    ? "refreshing"
    : uiError
      ? "error"
      : summary.status;
  const linkLabel = LINK_LABEL[displayStatus] ?? LINK_LABEL.error;

  return (
    <div className={`dashboard mode-${launchMode}`}>
      <div className="scanlines" aria-hidden />
      <div className="vignette" aria-hidden />
      <SideRail />

      <div className="stage">
        <header className="topbar">
          <div className="tb-left">
            <h1 className="tb-title">
              AI USAGE <span className="tb-title-tag">MONITOR</span>
            </h1>
            <div className="tb-meta">LAST SYNC {refreshedLabel} · {linkLabel}</div>
          </div>
          <SystemStrip status={displayStatus} message={uiError} />
          <ClockHeader />
        </header>

        <main className="panels" aria-busy={isRefreshing}>
          <ServicePanel kind="codex" title="CODEX" code="SYS-01" service={summary.services.codex} />
          <ServicePanel kind="claude" title="CLAUDE" code="SYS-02" service={summary.services.claude} />
          <ServicePanel
            kind="deepseek"
            title="DEEPSEEK"
            code="SYS-03"
            service={summary.services.deepseek}
          />
        </main>

        <TelemetryBar
          refreshedLabel={refreshedLabel}
          status={displayStatus}
          errorMessage={uiError}
          onRefresh={refresh}
        />
      </div>
    </div>
  );
}
