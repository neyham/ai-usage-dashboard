import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UsageSummary } from "./types";
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

export default function App() {
  const [summary, setSummary] = useState<UsageSummary>(EMPTY);

  const loadOnce = useCallback(async () => {
    try {
      setSummary(await invoke<UsageSummary>("get_summary"));
    } catch (err) {
      console.error("get_summary failed", err);
    }
  }, []);

  const refresh = useCallback(async () => {
    try {
      await invoke("refresh_now");
    } catch (err) {
      console.error("refresh_now failed", err);
    }
  }, []);

  useEffect(() => {
    loadOnce();
    refresh();

    const unlisten = listen<UsageSummary>("summary", (event) => setSummary(event.payload));

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") invoke("exit_app").catch(() => {});
      if (e.key === "F5") refresh();
    };
    window.addEventListener("keydown", onKey);

    return () => {
      unlisten.then((f) => f());
      window.removeEventListener("keydown", onKey);
    };
  }, [loadOnce, refresh]);

  // Screensaver mode: exit on any real input after a short arming delay.
  useEffect(() => {
    let cleanup = () => {};
    invoke<string>("launch_mode")
      .then((mode) => {
        if (mode !== "screensaver") return;
        let armed = false;
        let origin: { x: number; y: number } | null = null;
        const armTimer = window.setTimeout(() => (armed = true), 1200);
        const quit = () => invoke("exit_app").catch(() => {});
        const onMove = (e: MouseEvent) => {
          if (!armed) return;
          if (!origin) {
            origin = { x: e.screenX, y: e.screenY };
            return;
          }
          if (Math.abs(e.screenX - origin.x) + Math.abs(e.screenY - origin.y) > 8) quit();
        };
        const onAny = () => armed && quit();
        window.addEventListener("mousemove", onMove);
        window.addEventListener("mousedown", onAny);
        window.addEventListener("keydown", onAny);
        window.addEventListener("wheel", onAny);
        cleanup = () => {
          window.clearTimeout(armTimer);
          window.removeEventListener("mousemove", onMove);
          window.removeEventListener("mousedown", onAny);
          window.removeEventListener("keydown", onAny);
          window.removeEventListener("wheel", onAny);
        };
      })
      .catch(() => {});
    return () => cleanup();
  }, []);

  const refreshedLabel = summary.refreshedAt
    ? new Date(summary.refreshedAt).toLocaleTimeString("en-GB", {
        hour: "2-digit",
        minute: "2-digit",
      })
    : "--:--";

  return (
    <div className="dashboard">
      <div className="scanlines" aria-hidden />
      <div className="vignette" aria-hidden />
      <SideRail />

      <div className="stage">
        <header className="topbar">
          <div className="tb-left">
            <div className="tb-title">
              AI USAGE <span className="tb-title-tag">MONITOR</span>
            </div>
            <div className="tb-meta">LAST SYNC {refreshedLabel} · MAGI-LINK ACTIVE</div>
          </div>
          <SystemStrip status={summary.status} />
          <ClockHeader />
        </header>

        <main className="panels">
          <ServicePanel kind="codex" title="CODEX" code="SYS-01" service={summary.services.codex} />
          <ServicePanel kind="claude" title="CLAUDE" code="SYS-02" service={summary.services.claude} />
          <ServicePanel
            kind="deepseek"
            title="DEEPSEEK"
            code="SYS-03"
            service={summary.services.deepseek}
          />
        </main>

        <TelemetryBar refreshedLabel={refreshedLabel} onRefresh={refresh} />
      </div>
    </div>
  );
}
