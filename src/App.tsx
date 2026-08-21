import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  EnabledProviders,
  LaunchMode,
  SummaryStatus,
  UsageSummary,
  WindowMode,
} from "./types";
import { ClockHeader } from "./components/ClockHeader";
import { ServicePanel } from "./components/ServicePanel";
import { SystemStrip } from "./components/SystemStrip";
import { SideRail } from "./components/SideRail";
import { TelemetryBar } from "./components/TelemetryBar";
import { ProviderSettings } from "./components/ProviderSettings";
import { normalizeArtSkin, type ArtSkin } from "./ipTheme";
import {
  computePanelLayout,
  enabledProviderList,
  livePlanOf,
  loadStoredPlanSizes,
  mergeStoredPlanSizes,
  resolveSize,
  saveStoredPlanSizes,
  type PlanSize,
  type ProviderKind,
  type StoredPlanSizes,
} from "./planLayout";

const EMPTY: UsageSummary = {
  refreshedAt: null,
  status: "idle",
  enabledProviders: {
    codex: true,
    claude: true,
    deepseek: true,
    grok: false,
    cursor: false,
    antigravity: false,
  },
  services: {
    codex: { status: "AWAITING DATA", fromCache: false, dataMayBeStale: false },
    claude: { status: "AWAITING DATA", fromCache: false, dataMayBeStale: false },
    deepseek: { status: "AWAITING DATA", fromCache: false, dataMayBeStale: false },
    grok: { status: "AWAITING DATA", fromCache: false, dataMayBeStale: false },
    cursor: { status: "AWAITING DATA", fromCache: false, dataMayBeStale: false },
    antigravity: { status: "AWAITING DATA", fromCache: false, dataMayBeStale: false },
  },
};

const PANEL_META: Record<ProviderKind, { title: string; code: string }> = {
  codex: { title: "CODEX", code: "SYS-01" },
  claude: { title: "CLAUDE", code: "SYS-02" },
  deepseek: { title: "DEEPSEEK", code: "SYS-03" },
  grok: { title: "GROK", code: "SYS-04" },
  cursor: { title: "CURSOR", code: "SYS-05" },
  antigravity: { title: "ANTIGRAVITY", code: "SYS-06" },
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
  const [lowPowerMode, setLowPowerMode] = useState(false);
  const [artSkin, setArtSkin] = useState<ArtSkin>("ip");
  const [pageVisible, setPageVisible] = useState(() => !document.hidden);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [uiError, setUiError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [judgeDemo, setJudgeDemo] = useState(false);
  const [storedPlanSizes, setStoredPlanSizes] = useState<StoredPlanSizes>(() =>
    typeof localStorage === "undefined" ? {} : loadStoredPlanSizes(false),
  );
  const refreshPendingRef = useRef(false);
  const refreshTimerRef = useRef<number | null>(null);
  const settingsReturnFocusRef = useRef<HTMLElement | null>(null);

  const openSettings = useCallback(() => {
    const active = document.activeElement;
    settingsReturnFocusRef.current = active instanceof HTMLElement ? active : null;
    setSettingsOpen(true);
  }, []);

  const closeSettings = useCallback(() => {
    setSettingsOpen(false);
    window.requestAnimationFrame(() => {
      const previous = settingsReturnFocusRef.current;
      if (previous?.isConnected) {
        previous.focus();
      } else {
        document.querySelector<HTMLButtonElement>(".tm-settings")?.focus();
      }
    });
  }, []);

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
          if (event.payload.status === "refreshing") {
            setIsRefreshing(true);
          } else {
            finishRefresh();
          }
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
    let disposed = false;
    invoke<boolean>("low_power_mode")
      .then((enabled) => {
        if (!disposed) setLowPowerMode(enabled);
      })
      .catch((err) => console.error("low_power_mode failed", err));
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    invoke<string>("art_skin")
      .then((skin) => {
        if (!disposed) setArtSkin(normalizeArtSkin(skin));
      })
      .catch((err) => console.error("art_skin failed", err));
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    const onVisibilityChange = () => setPageVisible(!document.hidden);
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => document.removeEventListener("visibilitychange", onVisibilityChange);
  }, []);

  useEffect(() => {
    let disposed = false;
    invoke<boolean>("judge_demo")
      .then((enabled) => {
        if (!disposed) setJudgeDemo(enabled);
      })
      .catch((err) => console.error("judge_demo failed", err));
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    setStoredPlanSizes(loadStoredPlanSizes(judgeDemo));
  }, [judgeDemo]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && settingsOpen) {
        // The dialog owns Escape so it can keep itself mounted while saving.
        return;
      }
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
  }, [launchMode, refresh, settingsOpen]);

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
  const linkLabel =
    artSkin === "ip" && displayStatus === "ok"
      ? "LINK ACTIVE"
      : (LINK_LABEL[displayStatus] ?? LINK_LABEL.error);
  const enabledKinds = useMemo(
    () => enabledProviderList(summary.enabledProviders),
    [summary.enabledProviders],
  );
  const enabledCount = enabledKinds.length;
  const panelLayout = useMemo(() => {
    const sizes = {} as Record<ProviderKind, PlanSize>;
    const updates: StoredPlanSizes = {};
    for (const kind of enabledKinds) {
      const resolved = resolveSize(kind, livePlanOf(kind, summary.services), storedPlanSizes);
      sizes[kind] = resolved.size;
      if (resolved.next) {
        updates[kind] = resolved.next;
      }
    }
    return {
      sizes,
      updates,
      ...computePanelLayout(enabledKinds, sizes),
    };
  }, [enabledKinds, storedPlanSizes, summary.services]);

  useEffect(() => {
    if (Object.keys(panelLayout.updates).length === 0) {
      return;
    }
    const next = mergeStoredPlanSizes(storedPlanSizes, panelLayout.updates);
    saveStoredPlanSizes(judgeDemo, next);
    setStoredPlanSizes(next);
  }, [judgeDemo, panelLayout.updates, storedPlanSizes]);

  const saveSettings = useCallback(
    async (
      enabledProviders?: EnabledProviders,
      nextWindowMode?: WindowMode,
      deepseekApiKey?: string,
      nextLowPowerMode?: boolean,
      nextArtSkin?: ArtSkin,
    ) => {
      try {
        const args: {
          enabledProviders: EnabledProviders | null;
          windowMode: WindowMode | null;
          deepseekApiKey: string | null;
          lowPowerMode?: boolean;
          artSkin?: ArtSkin;
        } = {
          enabledProviders: enabledProviders ?? null,
          windowMode: nextWindowMode ?? null,
          deepseekApiKey: deepseekApiKey ?? null,
        };
        if (nextLowPowerMode !== undefined) args.lowPowerMode = nextLowPowerMode;
        if (nextArtSkin !== undefined) args.artSkin = nextArtSkin;
        const saved = await invoke<{
          enabledProviders: EnabledProviders;
          windowMode: WindowMode;
          lowPowerMode: boolean;
          artSkin: string;
        }>("save_display_settings", args);
        setSummary((current) => ({ ...current, enabledProviders: saved.enabledProviders }));
        setLaunchMode(saved.windowMode);
        setLowPowerMode(saved.lowPowerMode);
        setArtSkin(normalizeArtSkin(saved.artSkin));
      } catch (err) {
        try {
          const actualMode = await invoke<string>("launch_mode");
          if (isLaunchMode(actualMode)) setLaunchMode(actualMode);
        } catch (modeErr) {
          console.error("launch_mode reconciliation failed", modeErr);
        }
        throw err;
      }
    },
    [],
  );

  const effectiveWindowMode: WindowMode =
    launchMode === "fullscreen" ? "fullscreen" : "normal";

  return (
    <div
      className={`dashboard mode-${launchMode}${lowPowerMode ? " low-power" : ""}${pageVisible ? "" : " effects-paused"}`}
      data-low-power={lowPowerMode ? "true" : "false"}
      data-art={artSkin}
    >
      <div className="scanlines" aria-hidden />
      <div className="vignette" aria-hidden />
      <SideRail enabledProviders={summary.enabledProviders} />

      <div className="stage">
        <header className="topbar">
          <div className="tb-left">
            <h1 className="tb-title">
              AI USAGE <span className="tb-title-tag">MONITOR</span>
            </h1>
            <div className="tb-meta">LAST SYNC {refreshedLabel} · {linkLabel}</div>
          </div>
          <SystemStrip status={displayStatus} message={uiError} />
          <ClockHeader lowPowerMode={lowPowerMode} />
        </header>

        <main
          className="panels"
          data-count={enabledCount}
          data-layout={panelLayout.mode}
          aria-busy={isRefreshing}
        >
          {enabledKinds.map((kind) => (
            <ServicePanel
              key={kind}
              kind={kind}
              title={PANEL_META[kind].title}
              code={PANEL_META[kind].code}
              service={summary.services[kind]}
              planSize={panelLayout.sizes[kind]}
              slot={panelLayout.slots[kind]}
              placement={panelLayout.placements[kind]}
              artSkin={artSkin}
            />
          ))}
          {enabledCount === 0 && (
            <div className="panels-empty" role="status">
              <span>NO PROVIDERS SELECTED</span>
              <button type="button" onClick={openSettings}>
                OPEN SETTINGS
              </button>
            </div>
          )}
        </main>

        <TelemetryBar
          refreshedLabel={refreshedLabel}
          status={displayStatus}
          errorMessage={uiError}
          judgeDemo={judgeDemo}
          onRefresh={refresh}
          onSettings={launchMode === "screensaver" ? undefined : openSettings}
        />
      </div>
      {settingsOpen && launchMode !== "screensaver" && (
        <ProviderSettings
          value={summary.enabledProviders}
          windowMode={effectiveWindowMode}
          lowPowerMode={lowPowerMode}
          artSkin={artSkin}
          judgeDemo={judgeDemo}
          onClose={closeSettings}
          onSave={saveSettings}
        />
      )}
    </div>
  );
}
