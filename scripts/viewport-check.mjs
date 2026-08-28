import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const port = Number(process.env.UI_TEST_PORT ?? 1432);
const baseUrl = `http://127.0.0.1:${port}`;
const artifactDir =
  process.env.UI_ARTIFACT_DIR ?? join(tmpdir(), "ai-usage-dashboard-surface-results");
const readmeAssetDir = join(root, "docs", "assets");

const viewports = [
  { name: "default", width: 1600, height: 900 },
  { name: "surface-200-landscape", width: 1368, height: 912 },
  { name: "compact", width: 960, height: 540 },
  { name: "minimum", width: 640, height: 540 },
  { name: "surface-200-portrait", width: 912, height: 1368 },
  { name: "surface-snap-half", width: 684, height: 912 },
  { name: "surface-150-landscape", width: 1824, height: 1216 },
];

const baseSummary = {
  refreshedAt: "2026-07-10T04:30:00Z",
  status: "ok",
  enabledProviders: {
    codex: true,
    claude: true,
    deepseek: true,
    grok: true,
    cursor: false,
    antigravity: false,
  },
  services: {
    codex: {
      status: "NOMINAL",
      fromCache: false,
      dataMayBeStale: false,
      plan: "pro",
      fiveHourPercent: 18,
      sevenDayPercent: 61,
      fiveHourResetLocal: "07-29 12:30",
      sevenDayResetLocal: "07-29 01:02",
      resetCreditsAvailable: 3,
      resetCreditsExpireLocal: "08-15 12:00",
    },
    claude: {
      status: "NOMINAL",
      fromCache: false,
      dataMayBeStale: false,
      fiveHourPercent: 42,
      sevenDayPercent: 73,
      fiveHourResetLocal: "07-10 18:30",
      sevenDayResetLocal: "07-15 11:00",
    },
    deepseek: {
      status: "NOMINAL",
      fromCache: false,
      dataMayBeStale: false,
      currency: "CNY",
      balance: "17.80",
    },
    grok: {
      status: "NOMINAL",
      fromCache: false,
      dataMayBeStale: false,
      plan: "SuperGrok Heavy",
      usagePercent: 36,
      periodLabel: "7D",
      periodCaption: "WEEKLY WINDOW",
      usageResetLocal: "08-11 08:00",
      monthlyPercent: 28,
      monthlyResetLocal: "09-01 08:00",
      resetCreditsAvailable: 1,
      resetCreditsExpireLocal: "09-13 00:00",
    },
    cursor: {
      status: "NOMINAL",
      fromCache: false,
      dataMayBeStale: false,
      plan: "Ultra",
      usagePercent: 8,
      usageResetLocal: "08-13 09:13",
      includedPercent: 24,
      apiPercent: 41,
      grokBotPercent: 38,
      grokBotResetLocal: "09-01 08:00",
    },
    antigravity: {
      status: "NOMINAL",
      fromCache: false,
      dataMayBeStale: false,
      plan: "Google AI Pro",
      fiveHourPercent: 27,
      sevenDayPercent: 21,
      fiveHourResetLocal: "08-19 14:20",
      sevenDayResetLocal: "08-20 11:59",
    },
  },
};

const summaries = {
  normal: baseSummary,
  claude429: {
    ...baseSummary,
    status: "partial",
    services: {
      ...baseSummary.services,
      claude: {
        ...baseSummary.services.claude,
        status: "RATE LIMITED",
        fromCache: true,
        cooldownUntilLocal: "07-10 19:01",
      },
    },
  },
  failures: {
    ...baseSummary,
    status: "partial",
    services: {
      ...baseSummary.services,
      codex: {
        ...baseSummary.services.codex,
        status: "API ERROR",
        fromCache: true,
        dataMayBeStale: true,
      },
      deepseek: {
        ...baseSummary.services.deepseek,
        status: "API ERROR",
        fromCache: true,
        dataMayBeStale: true,
        balance: "12345678901234567890.12345678",
      },
    },
  },
  insufficient: {
    ...baseSummary,
    status: "partial",
    services: {
      ...baseSummary.services,
      deepseek: {
        ...baseSummary.services.deepseek,
        status: "INSUFFICIENT BALANCE",
        balance: "0.00",
      },
    },
  },
};

function startServer() {
  const viteEntry = resolve(root, "node_modules/vite/bin/vite.js");
  const child = spawn(
    process.execPath,
    [viteEntry, "preview", "--host", "127.0.0.1", "--port", String(port), "--strictPort"],
    { cwd: root, stdio: ["ignore", "pipe", "pipe"] },
  );
  let output = "";
  child.stdout.on("data", (chunk) => (output += chunk));
  child.stderr.on("data", (chunk) => (output += chunk));
  child.output = () => output;
  return child;
}

async function waitForServer(server) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (server.exitCode !== null) {
      throw new Error(`Vite exited before startup:\n${server.output()}`);
    }
    try {
      const response = await fetch(baseUrl);
      if (response.ok) return;
    } catch {
      // The server is still starting.
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw new Error(`Timed out waiting for ${baseUrl}:\n${server.output()}`);
}

async function installTauriMock(
  page,
  summary,
  launchMode = "normal",
  judgeDemo = false,
  initialLowPowerMode = false,
  initialArtSkin = "ip",
) {
  await page.addInitScript(
    ({ initialSummary, mode: initialMode, demo, initialLowPower, initialSkin }) => {
      let currentSummary = structuredClone(initialSummary);
      let mode = initialMode;
      let lowPowerMode = initialLowPower;
      let artSkin = String(initialSkin).trim().toLowerCase() === "ring" ? "ring" : "ip";
      let nextCallbackId = 1;
      let nextEventId = 1;
      const callbacks = new Map();
      const listeners = new Map();
      let refreshPending = false;
      let saveResolver = null;
      const stats = {
        exits: 0,
        refreshes: 0,
        savedSelections: [],
        saveCalls: [],
        deepseekKeySubmissions: 0,
        deepseekKeyLengths: [],
        deferSave: false,
        failNextSave: false,
        failNextSaveWithRollbackFailure: false,
        completeRefresh() {},
        finishSave() {},
      };

      const unregisterListener = (event, eventId) => {
        const eventListeners = listeners.get(event);
        eventListeners?.delete(eventId);
      };

      const emit = (event, payload) => {
        const eventListeners = listeners.get(event);
        if (!eventListeners) return;
        for (const [eventId, callbackId] of eventListeners) {
          callbacks.get(callbackId)?.({ event, id: eventId, payload });
        }
      };

      stats.completeRefresh = () => {
        if (!refreshPending) return;
        refreshPending = false;
        emit("summary", structuredClone(currentSummary));
      };
      stats.finishSave = () => {
        saveResolver?.();
        saveResolver = null;
      };

      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener };
      window.__TAURI_INTERNALS__ = {
        transformCallback(callback, once = false) {
          const id = nextCallbackId++;
          callbacks.set(id, (payload) => {
            callback(payload);
            if (once) callbacks.delete(id);
          });
          return id;
        },
        unregisterCallback(id) {
          callbacks.delete(id);
        },
        async invoke(command, args = {}) {
          if (command === "plugin:event|listen") {
            const eventId = nextEventId++;
            const eventListeners = listeners.get(args.event) ?? new Map();
            eventListeners.set(eventId, args.handler);
            listeners.set(args.event, eventListeners);
            return eventId;
          }
          if (command === "plugin:event|unlisten") {
            unregisterListener(args.event, args.eventId);
            return null;
          }
          if (command === "get_summary") return structuredClone(currentSummary);
          if (command === "launch_mode") return mode;
          if (command === "low_power_mode") return lowPowerMode;
          if (command === "art_skin") return artSkin;
          if (command === "judge_demo") return demo;
          if (command === "refresh_now") {
            stats.refreshes += 1;
            refreshPending = true;
            return true;
          }
          if (command === "save_display_settings") {
            const safeArgs = structuredClone(args);
            if (safeArgs.deepseekApiKey == null) {
              delete safeArgs.deepseekApiKey;
            } else {
              stats.deepseekKeySubmissions += 1;
              stats.deepseekKeyLengths.push(String(safeArgs.deepseekApiKey).length);
              safeArgs.deepseekApiKey = "[REDACTED]";
            }
            stats.saveCalls.push(safeArgs);
            if (stats.deferSave) {
              await new Promise((resolveSave) => {
                saveResolver = resolveSave;
              });
              stats.deferSave = false;
            }
            if (stats.failNextSave) {
              stats.failNextSave = false;
              if (stats.failNextSaveWithRollbackFailure && args.windowMode != null) {
                mode = String(args.windowMode).toLowerCase();
              }
              stats.failNextSaveWithRollbackFailure = false;
              throw new Error("CONFIG SAVE FAILED");
            }
            if (args.enabledProviders != null) {
              currentSummary = {
                ...currentSummary,
                enabledProviders: structuredClone(args.enabledProviders),
              };
              stats.savedSelections.push(structuredClone(args.enabledProviders));
              emit("summary", structuredClone(currentSummary));
            }
            if (args.windowMode != null) {
              const requested = String(args.windowMode).toLowerCase();
              if (requested !== "fullscreen" && requested !== "normal") {
                throw new Error("INVALID WINDOW MODE");
              }
              mode = requested;
            }
            if (args.lowPowerMode != null) {
              lowPowerMode = Boolean(args.lowPowerMode);
            }
            if (args.artSkin != null) {
              artSkin = String(args.artSkin).trim().toLowerCase() === "ring" ? "ring" : "ip";
            }
            return {
              enabledProviders: structuredClone(currentSummary.enabledProviders),
              windowMode: mode,
              lowPowerMode,
              artSkin,
            };
          }
          if (command === "exit_app") {
            stats.exits += 1;
            return null;
          }
          throw new Error(`Unexpected Tauri command: ${command}`);
        },
      };
      stats.setSummary = (next) => {
        currentSummary = structuredClone(next);
        emit("summary", structuredClone(currentSummary));
      };
      window.__DASHBOARD_TEST__ = stats;
    },
    {
      initialSummary: summary,
      mode: launchMode,
      demo: judgeDemo,
      initialLowPower: initialLowPowerMode,
      initialSkin: initialArtSkin,
    },
  );
}

async function legendWindows(page, panelSelector) {
  const bars = page.locator(`${panelSelector} .ip-bar`);
  if ((await bars.count()) > 0) {
    return bars.evaluateAll((els) =>
      els.map((el) => el.getAttribute("data-window")).filter(Boolean),
    );
  }
  return page
    .locator(`${panelSelector} .gauge-legend-item`)
    .evaluateAll((els) => els.map((el) => el.getAttribute("data-window")).filter(Boolean));
}

async function inspectLayout(page, viewport, { expectHiddenCursor = false } = {}) {
  return page.evaluate(({ width, height, expectHiddenCursor }) => {
    const issues = [];
    const root = document.documentElement;
    const tolerance = 1.5;

    if (root.scrollWidth > root.clientWidth + tolerance) {
      issues.push(`document horizontal overflow: ${root.scrollWidth} > ${root.clientWidth}`);
    }

    const selectors = [
      ".topbar",
      ".sysstrip",
      ".panels",
      ".panel",
      ".panel-head",
      ".panel-body",
      ".panel-status",
      ".chip",
      ".gauge-stage",
      ".balance-number",
      ".telemetry",
      ".tm-refresh",
      ".tm-settings",
    ];
    for (const selector of selectors) {
      for (const [index, element] of [...document.querySelectorAll(selector)].entries()) {
        const style = getComputedStyle(element);
        if (style.display === "none" || style.visibility === "hidden") continue;
        const rect = element.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) {
          issues.push(`${selector}[${index}] has no visible size`);
        }
        if (rect.left < -tolerance || rect.right > width + tolerance) {
          issues.push(
            `${selector}[${index}] crosses viewport horizontally: ${rect.left.toFixed(1)}..${rect.right.toFixed(1)}`,
          );
        }
      }
    }

    for (const [index, panel] of [...document.querySelectorAll(".panel")].entries()) {
      const panelRect = panel.getBoundingClientRect();
      for (const selector of [".panel-head", ".panel-body", ".panel-status", ".gauge-stage"]) {
        const child = panel.querySelector(selector);
        if (!child) continue;
        const rect = child.getBoundingClientRect();
        if (
          rect.left < panelRect.left - tolerance ||
          rect.right > panelRect.right + tolerance ||
          rect.top < panelRect.top - tolerance ||
          rect.bottom > panelRect.bottom + tolerance
        ) {
          issues.push(`${selector} escapes panel[${index}]`);
        }
      }

      const count = Number(document.querySelector(".panels")?.getAttribute("data-count") ?? 0);
      const gauge = panel.querySelector(".gauge-stage");
      if (gauge && getComputedStyle(gauge).display !== "none") {
        const gaugeRect = gauge.getBoundingClientRect();
        const diameter = Math.min(gaugeRect.width, gaugeRect.height);
        const minDiameter = height > 650 ? (count >= 5 ? 72 : 96) : 56;
        if (diameter + 0.5 < minDiameter) {
          issues.push(
            `panel[${index}] gauge is ${diameter.toFixed(1)}px, expected >= ${minDiameter}`,
          );
        }
        if (diameter < 120 && gauge.getAttribute("data-rings") !== "1") {
          issues.push(`panel[${index}] compact gauge still has inner ring`);
        }
      }

      const mascot = panel.querySelector(".panel-mascot");
      if (mascot && getComputedStyle(mascot).display !== "none") {
        const mascotRect = mascot.getBoundingClientRect();
        const minMascot = height > 650 ? (count >= 5 ? 48 : 72) : 40;
        if (mascotRect.height + 0.5 < minMascot) {
          issues.push(
            `panel[${index}] mascot is ${mascotRect.height.toFixed(1)}px tall, expected >= ${minMascot}`,
          );
        }
      }

      const readout = panel.querySelector(".gauge-readout");
      const chip = panel.querySelector(".chip");
      if (readout && chip) {
        const peakRect = readout.getBoundingClientRect();
        const chipRect = chip.getBoundingClientRect();
        const overlapW = Math.min(peakRect.right, chipRect.right) - Math.max(peakRect.left, chipRect.left);
        const overlapH = Math.min(peakRect.bottom, chipRect.bottom) - Math.max(peakRect.top, chipRect.top);
        if (overlapW > 2 && overlapH > 2) {
          issues.push(`gauge-readout overlaps chip in panel[${index}]`);
        }
      }

      const balance = panel.querySelector(".balance-number");
      if (balance && getComputedStyle(balance).display !== "none") {
        if (balance.getClientRects().length > 1) {
          issues.push(`balance-number wraps onto multiple lines in panel[${index}]`);
        }
      }
    }

    const refresh = document.querySelector(".tm-refresh");
    const refreshRect = refresh?.getBoundingClientRect();
    if (!refreshRect || refreshRect.height < 44 || refreshRect.width < 44) {
      issues.push(`refresh touch target is ${refreshRect?.width ?? 0}x${refreshRect?.height ?? 0}`);
    }
    const refreshCursor = refresh ? getComputedStyle(refresh).cursor : "";
    if (refresh && expectHiddenCursor && refreshCursor !== "none") {
      issues.push(`fullscreen refresh cursor is ${refreshCursor}`);
    } else if (refresh && !expectHiddenCursor && refreshCursor !== "pointer") {
      issues.push(`normal-mode refresh cursor is ${refreshCursor}`);
    }
    const settings = document.querySelector(".tm-settings");
    const settingsRect = settings?.getBoundingClientRect();
    if (!settingsRect || settingsRect.height < 44 || settingsRect.width < 44) {
      issues.push(`settings touch target is ${settingsRect?.width ?? 0}x${settingsRect?.height ?? 0}`);
    }
    const bodyCursor = getComputedStyle(document.body).cursor;
    const dashboard = document.querySelector(".dashboard");
    const dashboardCursor = dashboard ? getComputedStyle(dashboard).cursor : "";
    if (expectHiddenCursor && dashboardCursor !== "none") {
      issues.push(`fullscreen dashboard cursor is ${dashboardCursor}`);
    } else if (!expectHiddenCursor && bodyCursor === "none") {
      issues.push("normal-mode body cursor is hidden");
    }

    if (width >= 960 && height <= 540 && root.scrollHeight > height + tolerance) {
      issues.push(`minimum viewport scrolls vertically: ${root.scrollHeight} > ${height}`);
    }

    return issues;
  }, { ...viewport, expectHiddenCursor });
}

async function assertPlanPill(page, panelSelector, text) {
  const plan = page.locator(`${panelSelector} .panel-plan`);
  assert.equal((await plan.textContent())?.trim(), text, `${panelSelector} plan text`);
  const box = await plan.evaluate((el) => ({
    lines: el.getClientRects().length,
    whiteSpace: getComputedStyle(el).whiteSpace,
  }));
  assert.equal(box.whiteSpace, "nowrap", `${panelSelector} plan should not wrap`);
  assert.equal(box.lines, 1, `${panelSelector} plan "${text}" should stay on one line`);
}

async function assertIpTitles(page) {
  const titleColors = await page.locator(".panel-title").evaluateAll((els) =>
    els.map((el) => getComputedStyle(el).color),
  );
  assert.ok(titleColors.length >= 1, "expected panel titles");
  assert.ok(
    titleColors.every((color) => color === "rgb(255, 248, 240)"),
    `IP titles should be cream, got ${titleColors.join(" | ")}`,
  );
}

async function checkScenario(browser, stateName, summary, viewport) {
  const context = await browser.newContext({
    viewport,
    hasTouch: true,
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const page = await context.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await installTauriMock(page, summary);
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.locator(".panel").first().waitFor();
  await page.evaluate(() => document.fonts.ready);

  const expectedSystem = summary.status === "ok" ? "SYSTEM NOMINAL" : "WARNING";
  await assert.doesNotReject(() =>
    page.locator(".sys-text", { hasText: expectedSystem }).waitFor({ state: "attached" }),
  );

  const issues = await inspectLayout(page, viewport);
  assert.deepEqual(issues, [], `${stateName}/${viewport.name}:\n${issues.join("\n")}`);
  assert.deepEqual(pageErrors, [], `${stateName}/${viewport.name} page errors`);

  const refresh = page.locator(".tm-refresh");
  await refresh.click();
  await assert.doesNotReject(() => refresh.waitFor({ state: "visible" }));
  assert.equal(await refresh.isDisabled(), true, `${stateName}/${viewport.name} refresh busy`);
  await page.evaluate(() => window.__DASHBOARD_TEST__.completeRefresh());
  await assert.doesNotReject(() =>
    page.locator(".tm-refresh:not(:disabled)").waitFor({ timeout: 2_000 }),
  );

  const screenshot = join(artifactDir, `${stateName}-${viewport.name}.png`);
  await page.screenshot({ path: screenshot, fullPage: true });
  await context.close();
}

async function checkKeyboardAndScreensaver(browser) {
  const normalContext = await browser.newContext({
    viewport: { width: 1368, height: 912 },
    hasTouch: true,
  });
  const normalPage = await normalContext.newPage();
  await installTauriMock(normalPage, summaries.normal);
  await normalPage.goto(baseUrl, { waitUntil: "networkidle" });
  await normalPage.locator(".panel").first().waitFor();
  await normalPage.locator(".tm-settings").click();
  await normalPage.locator(".settings-dialog").waitFor();
  await normalPage.keyboard.press("Escape");
  assert.equal(await normalPage.locator(".settings-dialog").count(), 0);
  await normalPage.keyboard.press("Escape");
  assert.equal(await normalPage.evaluate(() => window.__DASHBOARD_TEST__.exits), 0);
  await normalPage.keyboard.press("F5");
  await normalPage.waitForFunction(() => window.__DASHBOARD_TEST__.refreshes === 1);
  await normalContext.close();

  const saverContext = await browser.newContext({
    viewport: { width: 912, height: 1368 },
    hasTouch: true,
  });
  const saverPage = await saverContext.newPage();
  await installTauriMock(saverPage, summaries.normal, "screensaver");
  await saverPage.goto(baseUrl, { waitUntil: "networkidle" });
  await saverPage.waitForTimeout(1_300);
  await saverPage.touchscreen.tap(100, 100);
  await saverPage.waitForFunction(() => window.__DASHBOARD_TEST__.exits === 1);
  assert.equal(
    await saverPage.locator(".dashboard").evaluate((element) => getComputedStyle(element).cursor),
    "none",
  );
  assert.equal(await saverPage.locator(".tm-settings").count(), 0);
  await saverContext.close();
}

async function checkProviderSelection(browser) {
  const context = await browser.newContext({
    viewport: { width: 1368, height: 912 },
    hasTouch: true,
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const page = await context.newPage();
  await installTauriMock(page, summaries.normal);
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.locator(".panel").first().waitFor();
  assert.equal(await page.locator(".panel").count(), 4);
  assert.deepEqual(await legendWindows(page, ".panel-grok"), ["7D", "MONTH"]);
  assert.equal(await page.locator(".panel-grok .panel-plan").textContent(), "SUPERGROK HEAVY");

  await page.locator(".tm-settings").click();
  await page.locator(".settings-dialog").waitFor();
  assert.equal(await page.locator(".provider-grid .provider-option").count(), 6);
  assert.equal(await page.locator('.provider-option input[type="checkbox"]:checked').count(), 4);
  assert.equal(await page.locator('.window-mode-option input[value="normal"]').isChecked(), true);
  await page.screenshot({ path: join(artifactDir, "provider-settings.png"), fullPage: true });
  await page.locator(".window-mode-option", { hasText: "FULLSCREEN" }).click();
  await page.locator(".provider-option.provider-claude").click();
  await page.locator(".settings-save").click();
  await page.locator(".settings-dialog").waitFor({ state: "detached" });
  assert.equal(
    await page.locator(".dashboard").evaluate((el) => el.classList.contains("mode-fullscreen")),
    true,
  );
  assert.equal(await page.locator(".panel").count(), 3);
  assert.equal(await page.locator(".panel-claude").count(), 0);
  // Return to windowed so later layout checks keep an interactive cursor.
  await page.locator(".tm-settings").click();
  await page.locator(".settings-dialog").waitFor();
  await page.locator(".window-mode-option", { hasText: "WINDOWED" }).click();
  await page.locator(".settings-save").click();
  await page.locator(".settings-dialog").waitFor({ state: "detached" });
  assert.equal(
    await page.locator(".dashboard").evaluate((el) => el.classList.contains("mode-fullscreen")),
    false,
  );
  assert.deepEqual(await inspectLayout(page, { width: 1368, height: 912 }), []);
  await page.screenshot({ path: join(artifactDir, "provider-selection-three.png"), fullPage: true });

  await page.locator(".tm-settings").click();
  await page.locator(".provider-option.provider-deepseek").click();
  await page.locator(".settings-save").click();
  assert.equal(await page.locator(".panel").count(), 2);
  assert.deepEqual(await inspectLayout(page, { width: 1368, height: 912 }), []);
  await page.screenshot({ path: join(artifactDir, "provider-selection-two.png"), fullPage: true });

  await page.locator(".tm-settings").click();
  await page.locator(".provider-option.provider-codex").click();
  await page.locator(".settings-save").click();
  await page.locator(".panel-grok").waitFor();
  assert.equal(await page.locator(".panel").count(), 1);
  assert.deepEqual(await inspectLayout(page, { width: 1368, height: 912 }), []);

  await page.locator(".tm-settings").click();
  await page.locator(".provider-option.provider-grok").click();
  await page.locator(".settings-save").click();
  await page.locator(".panels-empty").waitFor();
  assert.equal(await page.locator(".panel").count(), 0);

  await page.locator(".panels-empty button").click();
  await page.locator(".provider-option.provider-codex").click();
  await page.locator(".settings-save").click();
  await page.locator(".panel-codex").waitFor();
  assert.equal(await page.locator(".panel").count(), 1);
  assert.deepEqual(await inspectLayout(page, { width: 1368, height: 912 }), []);

  await page.locator(".tm-settings").click();
  await page.locator(".provider-option.provider-claude").click();
  await page.locator(".provider-option.provider-deepseek").click();
  await page.locator(".provider-option.provider-grok").click();
  await page.locator(".settings-save").click();
  assert.equal(await page.locator(".panel").count(), 4);
  assert.deepEqual(await inspectLayout(page, { width: 1368, height: 912 }), []);

  const saves = await page.evaluate(() => window.__DASHBOARD_TEST__.savedSelections);
  assert.deepEqual(saves, [
    { codex: true, claude: false, deepseek: true, grok: true, cursor: false, antigravity: false }, // fullscreen + uncheck Claude
    { codex: true, claude: false, deepseek: false, grok: true, cursor: false, antigravity: false },
    { codex: false, claude: false, deepseek: false, grok: true, cursor: false, antigravity: false },
    { codex: false, claude: false, deepseek: false, grok: false, cursor: false, antigravity: false },
    { codex: true, claude: false, deepseek: false, grok: false, cursor: false, antigravity: false },
    { codex: true, claude: true, deepseek: true, grok: true, cursor: false, antigravity: false },
  ]);
  const saveCalls = await page.evaluate(() => window.__DASHBOARD_TEST__.saveCalls);
  assert.deepEqual(saveCalls[0], {
    enabledProviders: { codex: true, claude: false, deepseek: true, grok: true, cursor: false, antigravity: false },
    windowMode: "fullscreen",
  });
  assert.deepEqual(saveCalls[1], { enabledProviders: null, windowMode: "normal" });
  await page.screenshot({ path: join(artifactDir, "provider-selection.png"), fullPage: true });
  await context.close();
}

async function checkFullscreenThreeColumns(browser) {
  const summary = {
    ...baseSummary,
    enabledProviders: {
      ...baseSummary.enabledProviders,
      claude: false,
    },
  };
  const fullscreenViewports = [
    { name: "surface-200", width: 1368, height: 912, deviceScaleFactor: 2 },
    { name: "surface-150", width: 1824, height: 1216, deviceScaleFactor: 1.5 },
    { name: "full-hd", width: 1920, height: 1080, deviceScaleFactor: 1 },
  ];

  for (const viewport of fullscreenViewports) {
    const context = await browser.newContext({
      viewport: { width: viewport.width, height: viewport.height },
      deviceScaleFactor: viewport.deviceScaleFactor,
      hasTouch: true,
      reducedMotion: "reduce",
      colorScheme: "dark",
    });
    const page = await context.newPage();
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));
    await installTauriMock(page, summary, "fullscreen");
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.locator(".dashboard.mode-fullscreen").waitFor();
    assert.equal(await page.locator(".panel").count(), 3);

    const grid = await page.locator(".panels").evaluate((element) => {
      const panels = [...element.querySelectorAll(".panel")].map((panel) =>
        panel.getBoundingClientRect(),
      );
      return {
        columns: getComputedStyle(element).gridTemplateColumns
          .split(/\s+/)
          .filter(Boolean).length,
        lefts: panels.map((panel) => panel.left),
        tops: panels.map((panel) => panel.top),
      };
    });
    assert.equal(grid.columns, 3, `${viewport.name} fullscreen column count`);
    assert.equal(
      new Set(grid.lefts.map((left) => Math.round(left))).size,
      3,
      `${viewport.name} fullscreen panel columns`,
    );
    assert.ok(
      Math.max(...grid.tops) - Math.min(...grid.tops) <= 1,
      `${viewport.name} fullscreen panels must share one row`,
    );
    assert.deepEqual(
      await inspectLayout(
        page,
        { width: viewport.width, height: viewport.height },
        { expectHiddenCursor: false },
      ),
      [],
    );
    assert.deepEqual(pageErrors, [], `${viewport.name} fullscreen page errors`);

    if (viewport.name === "surface-200") {
      await page.screenshot({
        path: join(artifactDir, "fullscreen-three-columns.png"),
        fullPage: true,
      });
    }
    await context.close();
  }
}

async function checkSettingsTransactions(browser) {
  const cliContext = await browser.newContext({ viewport: { width: 1368, height: 912 } });
  const cliPage = await cliContext.newPage();
  await installTauriMock(cliPage, summaries.normal, "fullscreen");
  await cliPage.goto(baseUrl, { waitUntil: "networkidle" });
  await cliPage.locator(".dashboard.mode-fullscreen").waitFor();
  assert.equal(
    await cliPage.locator(".tm-settings").evaluate((element) => getComputedStyle(element).cursor),
    "pointer",
  );

  await cliPage.locator(".tm-settings").click();
  await cliPage.waitForFunction(() =>
    document.activeElement?.classList.contains("settings-close"),
  );
  await cliPage.keyboard.press("Shift+Tab");
  assert.equal(
    await cliPage.evaluate(() => document.activeElement?.classList.contains("settings-save")),
    true,
  );
  await cliPage.keyboard.press("Tab");
  assert.equal(
    await cliPage.evaluate(() => document.activeElement?.classList.contains("settings-close")),
    true,
  );
  await cliPage.keyboard.press("Escape");
  await cliPage.locator(".settings-dialog").waitFor({ state: "detached" });
  await cliPage.waitForFunction(() =>
    document.activeElement?.classList.contains("tm-settings"),
  );

  // A provider-only save from a CLI fullscreen launch must not persist that
  // process-only override as the window preference.
  await cliPage.locator(".tm-settings").click();
  await cliPage.locator(".provider-option.provider-claude").click();
  await cliPage.locator(".settings-save").click();
  await cliPage.locator(".settings-dialog").waitFor({ state: "detached" });
  const cliSave = await cliPage.evaluate(() => window.__DASHBOARD_TEST__.saveCalls[0]);
  assert.deepEqual(cliSave, {
    enabledProviders: { codex: true, claude: false, deepseek: true, grok: true, cursor: false, antigravity: false },
    windowMode: null,
  });
  assert.equal(await cliPage.locator(".dashboard.mode-fullscreen").count(), 1);

  // Explicitly activating the already-selected effective CLI mode lets the
  // user promote it to the persisted preference without a toggle round-trip.
  await cliPage.locator(".tm-settings").click();
  await cliPage
    .locator('label.window-mode-option:has(input[name="windowMode"][value="fullscreen"])')
    .click();
  await cliPage.locator(".settings-save").click();
  await cliPage.locator(".settings-dialog").waitFor({ state: "detached" });
  const promotedCliSave = await cliPage.evaluate(() => window.__DASHBOARD_TEST__.saveCalls[1]);
  assert.deepEqual(promotedCliSave, {
    enabledProviders: null,
    windowMode: "fullscreen",
  });
  await cliContext.close();

  const failureContext = await browser.newContext({ viewport: { width: 1368, height: 912 } });
  const failurePage = await failureContext.newPage();
  await installTauriMock(failurePage, summaries.normal);
  await failurePage.goto(baseUrl, { waitUntil: "networkidle" });
  await failurePage.locator(".panel").first().waitFor();

  await failurePage.evaluate(() => {
    window.__DASHBOARD_TEST__.deferSave = true;
  });
  await failurePage.locator(".tm-settings").click();
  await failurePage.locator(".provider-option.provider-claude").click();
  await failurePage.locator(".settings-save").click();
  await failurePage.locator(".settings-save", { hasText: "SAVING" }).waitFor();
  await failurePage.keyboard.press("Escape");
  assert.equal(await failurePage.locator(".settings-dialog").count(), 1);
  await failurePage.evaluate(() => window.__DASHBOARD_TEST__.finishSave());
  await failurePage.locator(".settings-dialog").waitFor({ state: "detached" });
  assert.equal(await failurePage.locator(".panel-claude").count(), 0);

  await failurePage.evaluate(() => {
    window.__DASHBOARD_TEST__.failNextSave = true;
  });
  await failurePage.locator(".tm-settings").click();
  await failurePage.locator(".provider-option.provider-claude").click();
  await failurePage.locator(".settings-save").click();
  await failurePage.locator('[role="alert"]', { hasText: "SETTINGS SAVE FAILED" }).waitFor();
  assert.equal(await failurePage.locator(".settings-dialog").count(), 1);
  assert.equal(await failurePage.locator(".panel-claude").count(), 0);
  await failurePage.keyboard.press("Escape");
  await failurePage.locator(".settings-dialog").waitFor({ state: "detached" });

  await failurePage.evaluate(() => {
    window.__DASHBOARD_TEST__.failNextSave = true;
    window.__DASHBOARD_TEST__.failNextSaveWithRollbackFailure = true;
  });
  await failurePage.locator(".tm-settings").click();
  await failurePage
    .locator('label.window-mode-option:has(input[name="windowMode"][value="fullscreen"])')
    .click();
  await failurePage.locator(".settings-save").click();
  await failurePage.locator('[role="alert"]', { hasText: "SETTINGS SAVE FAILED" }).waitFor();
  await failurePage.locator(".dashboard.mode-fullscreen").waitFor();
  assert.equal(await failurePage.locator(".settings-dialog").count(), 1);
  await failurePage.keyboard.press("Escape");
  await failurePage.locator(".settings-dialog").waitFor({ state: "detached" });
  await failurePage.keyboard.press("Escape");
  await failurePage.waitForFunction(() => window.__DASHBOARD_TEST__.exits === 1);
  await failureContext.close();
}

async function checkDeepSeekKeySettings(browser) {
  const context = await browser.newContext({ viewport: { width: 1368, height: 912 } });
  const page = await context.newPage();
  await installTauriMock(page, summaries.normal);
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.locator(".panel").first().waitFor();

  await page.locator(".tm-settings").click();
  const keyInput = page.locator("#deepseek-api-key");
  assert.equal(await keyInput.getAttribute("type"), "password");
  assert.equal(await keyInput.getAttribute("autocomplete"), "new-password");
  assert.equal(await keyInput.inputValue(), "");
  await page.locator(".settings-save").click();
  await page.locator(".settings-dialog").waitFor({ state: "detached" });
  assert.equal(await page.evaluate(() => window.__DASHBOARD_TEST__.saveCalls.length), 0);

  const fixtureKey = "sk-viewport-fixture";
  await page.locator(".tm-settings").click();
  await page.locator("#deepseek-api-key").fill(fixtureKey);
  await page.locator(".settings-save").click();
  await page.locator(".settings-dialog").waitFor({ state: "detached" });
  assert.deepEqual(
    await page.evaluate(() => ({
      submissions: window.__DASHBOARD_TEST__.deepseekKeySubmissions,
      lengths: window.__DASHBOARD_TEST__.deepseekKeyLengths,
      call: window.__DASHBOARD_TEST__.saveCalls[0],
    })),
    {
      submissions: 1,
      lengths: [fixtureKey.length],
      call: {
        enabledProviders: null,
        windowMode: null,
        deepseekApiKey: "[REDACTED]",
      },
    },
  );
  assert.equal((await page.locator("body").innerText()).includes(fixtureKey), false);

  await page.locator(".tm-settings").click();
  assert.equal(await page.locator("#deepseek-api-key").inputValue(), "");
  await page.keyboard.press("Escape");
  await page.locator(".settings-dialog").waitFor({ state: "detached" });

  await page.evaluate(() => {
    window.__DASHBOARD_TEST__.failNextSave = true;
  });
  await page.locator(".tm-settings").click();
  await page.locator("#deepseek-api-key").fill("sk-retry-fixture");
  await page.locator(".provider-option.provider-claude").click();
  await page.locator(".settings-save").click();
  await page.locator('[role="alert"]', { hasText: "SETTINGS SAVE FAILED" }).waitFor();
  assert.equal(await page.locator(".settings-dialog").count(), 1);
  assert.equal(await page.locator(".panel-claude").count(), 1);
  assert.equal(await page.locator("#deepseek-api-key").inputValue(), "sk-retry-fixture");
  await page.keyboard.press("Escape");
  await page.locator(".settings-dialog").waitFor({ state: "detached" });
  await context.close();
}

async function checkAuthStatusChips(browser) {
  const context = await browser.newContext({ viewport: { width: 1368, height: 912 } });
  const page = await context.newPage();
  const authSummary = {
    ...baseSummary,
    status: "error",
    services: {
      codex: { ...baseSummary.services.codex, status: "LOGIN REQUIRED", fromCache: true },
      claude: { ...baseSummary.services.claude, status: "AUTH EXPIRED", dataMayBeStale: true },
      deepseek: { ...baseSummary.services.deepseek, status: "KEY MISSING", fromCache: true },
      grok: { ...baseSummary.services.grok, status: "CREDENTIAL ERROR", dataMayBeStale: true },
      cursor: { ...baseSummary.services.cursor, status: "SESSION EXPIRED", fromCache: true },
    },
  };
  await installTauriMock(page, authSummary);
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.locator(".panel").first().waitFor();
  assert.equal(await page.locator(".chip-auth").count(), 4);
  assert.deepEqual(await page.locator(".chip-auth .chip-tag").allTextContents(), [
    "AUTH",
    "AUTH",
    "AUTH",
    "AUTH",
  ]);
  await context.close();

  for (const status of [
    "GROK SESSION EXPIRED",
    "GROK ACCESS UNAVAILABLE",
    "TEAM USAGE UNAVAILABLE",
    "SESSION EXPIRED",
  ]) {
    const grokContext = await browser.newContext({ viewport: { width: 1368, height: 912 } });
    const grokPage = await grokContext.newPage();
    const grokAuthSummary = {
      ...baseSummary,
      status: "error",
      enabledProviders: {
        ...baseSummary.enabledProviders,
        grok: status !== "SESSION EXPIRED",
        cursor: status === "SESSION EXPIRED",
      },
      services: {
        ...baseSummary.services,
        grok: {
          ...baseSummary.services.grok,
          status: status === "SESSION EXPIRED" ? "NOMINAL" : status,
          fromCache: status !== "SESSION EXPIRED",
          dataMayBeStale: status !== "SESSION EXPIRED",
        },
        cursor: {
          ...baseSummary.services.cursor,
          status: status === "SESSION EXPIRED" ? status : "NOMINAL",
          fromCache: status === "SESSION EXPIRED",
          dataMayBeStale: status === "SESSION EXPIRED",
        },
      },
    };
    await installTauriMock(grokPage, grokAuthSummary);
    await grokPage.goto(baseUrl, { waitUntil: "networkidle" });
    const chip = grokPage.locator(
      status === "SESSION EXPIRED" ? ".panel-cursor .chip" : ".panel-grok .chip",
    );
    await chip.waitFor();
    assert.equal(await chip.getAttribute("class"), "chip chip-auth");
    assert.equal(await chip.locator(".chip-tag").textContent(), "AUTH");
    assert.equal(await chip.locator(".chip-text").textContent(), status);
    assert.equal(await chip.getAttribute("aria-label"), `AUTH: ${status}`);
    await grokContext.close();
  }
}

async function checkCursorPanel(browser) {
  const fourPanel = {
    ...baseSummary,
    enabledProviders: {
      ...baseSummary.enabledProviders,
      grok: false,
      cursor: true,
    },
  };
  const fivePanel = {
    ...baseSummary,
    enabledProviders: {
      ...baseSummary.enabledProviders,
      cursor: true,
    },
  };
  const fiveHeroPanel = {
    ...fivePanel,
    services: {
      ...fivePanel.services,
      cursor: { ...fivePanel.services.cursor, plan: "Pro" },
    },
  };

  const contentContext = await browser.newContext({
    viewport: { width: 1600, height: 900 },
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const contentPage = await contentContext.newPage();
  await installTauriMock(contentPage, fourPanel);
  await contentPage.goto(baseUrl, { waitUntil: "networkidle" });
  await contentPage.locator(".panel-cursor").waitFor();
  assert.equal(await contentPage.locator(".panel").count(), 4);
  assert.equal(await contentPage.locator(".panel-cursor .panel-plan").textContent(), "ULTRA");
  assert.equal(await contentPage.locator(".panel-cursor").getAttribute("data-meter"), "ring");
  assert.deepEqual(await legendWindows(contentPage, ".panel-cursor"), [
    "INCLUDED",
    "AUTO",
    "API",
    "GROK BOT",
  ]);
  const cursorLegend = (await contentPage.locator(".panel-cursor .gauge-legend").textContent()) ?? "";
  assert.match(cursorLegend, /INCLUDED 24/);
  assert.match(cursorLegend, /AUTO 8/);
  assert.match(cursorLegend, /API 41/);
  assert.match(cursorLegend, /GROK BOT 38/);
  assert.equal(
    (cursorLegend.match(/08-13 09:13/g) ?? []).length,
    1,
    "Included/Auto share one billing-cycle reset",
  );
  assert.deepEqual(await inspectLayout(contentPage, { width: 1600, height: 900 }), []);
  await contentContext.close();

  const layoutContext = await browser.newContext({
    viewport: { width: 1824, height: 1216 },
    deviceScaleFactor: 1.5,
    hasTouch: true,
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const layoutPage = await layoutContext.newPage();
  await installTauriMock(layoutPage, fivePanel);
  await layoutPage.goto(baseUrl, { waitUntil: "networkidle" });
  await layoutPage.locator(".panel-cursor").waitFor();
  assert.equal(await layoutPage.locator(".panel").count(), 5);
  assert.equal(await layoutPage.locator(".panels").getAttribute("data-layout"), "five-split");
  const splitColumns = await layoutPage.locator(".panels").evaluate(
    (element) => getComputedStyle(element).gridTemplateColumns.split(/\s+/).filter(Boolean).length,
  );
  assert.equal(splitColumns, 12, "five-panel split uses a twelve-track grid");
  assert.equal(await layoutPage.locator(".panel-grok").getAttribute("data-slot"), "top-left");
  assert.equal(await layoutPage.locator(".panel-cursor").getAttribute("data-slot"), "top-right");
  assert.equal(await layoutPage.locator(".panel-grok").getAttribute("data-plan-size"), "L");
  assert.equal(await layoutPage.locator(".panel-cursor").getAttribute("data-plan-size"), "L");
  const cursorResets = (await layoutPage.locator(".panel-cursor .ip-bar-reset").allTextContents())
    .map((text) => text.replace(/\s+/g, " ").trim())
    .filter(Boolean);
  assert.deepEqual(cursorResets, ["RESET 08-13 09:13", "RESET 09-01 08:00"]);
  assert.deepEqual(await inspectLayout(layoutPage, { width: 1824, height: 1216 }), []);
  await layoutPage.screenshot({ path: join(artifactDir, "cursor-five-panels.png"), fullPage: true });
  await layoutContext.close();

  const heroContext = await browser.newContext({
    viewport: { width: 1600, height: 900 },
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const heroPage = await heroContext.newPage();
  await installTauriMock(heroPage, fiveHeroPanel);
  await heroPage.goto(baseUrl, { waitUntil: "networkidle" });
  await heroPage.locator(".panel-grok").waitFor();
  assert.equal(await heroPage.locator(".panels").getAttribute("data-layout"), "five-hero");
  assert.equal(await heroPage.locator(".panel-grok").getAttribute("data-slot"), "hero");
  assert.equal(await heroPage.locator(".panel-codex").getAttribute("data-slot"), "a");
  assert.equal(await heroPage.locator(".panel-claude").getAttribute("data-slot"), "b");
  assert.equal(await heroPage.locator(".panel-cursor").getAttribute("data-slot"), "c");
  assert.equal(await heroPage.locator(".panel-deepseek").getAttribute("data-slot"), "d");
  assert.equal(await heroPage.locator(".panel-cursor").getAttribute("data-plan-size"), "M");
  const heroGeometry = await heroPage.evaluate(() => {
    const board = document.querySelector(".panels");
    const hero = document.querySelector(".panel-grok");
    const topRight = document.querySelector(".panel-claude");
    if (!board || !hero || !topRight) return null;
    const boardRect = board.getBoundingClientRect();
    const heroRect = hero.getBoundingClientRect();
    const topRightRect = topRight.getBoundingClientRect();
    return {
      heroWidthRatio: heroRect.width / boardRect.width,
      heroHeightRatio: heroRect.height / boardRect.height,
      sideIsRight: topRightRect.left >= heroRect.right - 2,
      fillsRight: Math.abs(topRightRect.right - boardRect.right) < 4,
    };
  });
  assert.ok(heroGeometry, "five-hero geometry is measurable");
  assert.ok(
    heroGeometry.heroWidthRatio > 0.45 && heroGeometry.heroWidthRatio < 0.55,
    `hero is about half width, got ${heroGeometry.heroWidthRatio}`,
  );
  assert.ok(heroGeometry.heroHeightRatio > 0.9, `hero spans both rows, got ${heroGeometry.heroHeightRatio}`);
  assert.equal(heroGeometry.sideIsRight, true, "top-right tile sits beside the hero");
  assert.equal(heroGeometry.fillsRight, true, "top-right tile reaches the board edge");
  await heroPage.screenshot({ path: join(artifactDir, "five-hero-panels.png"), fullPage: true });
  assert.deepEqual(await inspectLayout(heroPage, { width: 1600, height: 900 }), []);
  await heroContext.close();

  const fillPanel = {
    ...fiveHeroPanel,
    services: {
      ...fiveHeroPanel.services,
      grok: { ...fiveHeroPanel.services.grok, plan: "SuperGrok" },
    },
  };
  const fillContext = await browser.newContext({
    viewport: { width: 1600, height: 900 },
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const fillPage = await fillContext.newPage();
  await installTauriMock(fillPage, fillPanel);
  await fillPage.goto(baseUrl, { waitUntil: "networkidle" });
  await fillPage.locator(".panel-grok").waitFor();
  assert.equal(await fillPage.locator(".panels").getAttribute("data-layout"), "five-fill");
  assert.equal(await fillPage.locator(".panel-codex").getAttribute("data-plan-size"), "M");
  assert.equal(await fillPage.locator(".panel-claude").getAttribute("data-plan-size"), "M");
  assert.equal(await fillPage.locator(".panel-deepseek").getAttribute("data-plan-size"), "S");
  assert.equal(await fillPage.locator(".panel-grok").getAttribute("data-plan-size"), "M");
  const fillRight = await fillPage.evaluate(() => {
    const board = document.querySelector(".panels")?.getBoundingClientRect();
    const last = document.querySelector(".panel-cursor")?.getBoundingClientRect();
    if (!board || !last) return null;
    return Math.abs(last.right - board.right);
  });
  assert.ok(fillRight != null && fillRight < 3, "five-fill last panel reaches the right edge");
  assert.deepEqual(await inspectLayout(fillPage, { width: 1600, height: 900 }), []);
  await fillContext.close();

  const persistContext = await browser.newContext({
    viewport: { width: 1600, height: 900 },
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const persistPage = await persistContext.newPage();
  const pendingHero = {
    ...fiveHeroPanel,
    services: {
      ...fiveHeroPanel.services,
      grok: { ...fiveHeroPanel.services.grok, plan: undefined },
    },
  };
  await installTauriMock(persistPage, pendingHero);
  await persistPage.goto(baseUrl, { waitUntil: "networkidle" });
  await persistPage.locator(".panel-grok").waitFor();
  assert.equal(await persistPage.locator(".panel-grok").getAttribute("data-plan-size"), "M");
  assert.equal(await persistPage.locator(".panels").getAttribute("data-layout"), "five-fill");
  await persistPage.evaluate((next) => window.__DASHBOARD_TEST__.setSummary(next), fiveHeroPanel);
  await persistPage.waitForFunction(
    () => document.querySelector(".panel-grok")?.getAttribute("data-plan-size") === "L",
  );
  assert.equal(await persistPage.locator(".panels").getAttribute("data-layout"), "five-hero");
  await persistPage.evaluate((next) => window.__DASHBOARD_TEST__.setSummary(next), pendingHero);
  await persistPage.waitForFunction(
    () => document.querySelector(".panels")?.getAttribute("data-layout") === "five-hero",
  );
  assert.equal(await persistPage.locator(".panel-grok").getAttribute("data-plan-size"), "L");
  await persistContext.close();
}

async function checkAntigravityPanel(browser) {
  const fourPanel = {
    ...baseSummary,
    enabledProviders: {
      ...baseSummary.enabledProviders,
      grok: false,
      antigravity: true,
    },
  };
  const sixPanel = {
    ...baseSummary,
    enabledProviders: {
      ...baseSummary.enabledProviders,
      cursor: true,
      antigravity: true,
    },
  };

  const contentContext = await browser.newContext({
    viewport: { width: 1600, height: 900 },
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const contentPage = await contentContext.newPage();
  await installTauriMock(contentPage, fourPanel);
  await contentPage.goto(baseUrl, { waitUntil: "networkidle" });
  await contentPage.locator(".panel-antigravity").waitFor();
  assert.equal(await contentPage.locator(".panel").count(), 4);
  await assertPlanPill(contentPage, ".panel-antigravity", "GOOGLE AI PRO");
  assert.deepEqual(await legendWindows(contentPage, ".panel-antigravity"), ["7D", "5H"]);
  assert.match(
    (await contentPage.locator(".panel-antigravity .gauge-legend").textContent()) ?? "",
    /7D 21/,
  );
  assert.match(
    (await contentPage.locator(".panel-antigravity .gauge-legend").textContent()) ?? "",
    /08-20 11:59/,
  );
  assert.doesNotMatch(
    (await contentPage.locator(".panel-antigravity .gauge-legend").textContent()) ?? "",
    /RESET/,
  );
  assert.match(
    (await contentPage.locator(".panel-antigravity .gauge-readout").textContent()) ?? "",
    /21/,
  );
  assert.deepEqual(await inspectLayout(contentPage, { width: 1600, height: 900 }), []);
  await contentContext.close();

  const layoutContext = await browser.newContext({
    viewport: { width: 1824, height: 1216 },
    deviceScaleFactor: 1.5,
    hasTouch: true,
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const layoutPage = await layoutContext.newPage();
  await installTauriMock(layoutPage, sixPanel);
  await layoutPage.goto(baseUrl, { waitUntil: "networkidle" });
  await layoutPage.locator(".panel-antigravity").waitFor();
  assert.equal(await layoutPage.locator(".panel").count(), 6);
  const columns = await layoutPage.locator(".panels").evaluate(
    (element) => getComputedStyle(element).gridTemplateColumns.split(/\s+/).filter(Boolean).length,
  );
  assert.equal(columns, 3, "six-panel wrap uses three columns");
  assert.deepEqual(await inspectLayout(layoutPage, { width: 1824, height: 1216 }), []);
  await layoutPage.screenshot({
    path: join(artifactDir, "antigravity-six-panels.png"),
    fullPage: true,
  });
  await layoutContext.close();

  const compactContext = await browser.newContext({
    viewport: { width: 640, height: 540 },
    hasTouch: true,
    reducedMotion: "reduce",
    colorScheme: "dark",
  });
  const compactPage = await compactContext.newPage();
  await installTauriMock(compactPage, sixPanel);
  await compactPage.goto(baseUrl, { waitUntil: "networkidle" });
  await compactPage.locator(".panel-antigravity").waitFor();
  assert.equal(await compactPage.locator(".panel").count(), 6);
  assert.deepEqual(await inspectLayout(compactPage, { width: 640, height: 540 }), []);
  await compactContext.close();
}

async function checkRateLimitStatusChips(browser) {
  for (const fromCache of [false, true]) {
    const context = await browser.newContext({ viewport: { width: 1368, height: 912 } });
    const page = await context.newPage();
    const rateLimitedSummary = {
      ...baseSummary,
      status: "error",
      services: {
        ...baseSummary.services,
        codex: {
          ...baseSummary.services.codex,
          status: "RATE LIMITED",
          fromCache,
          dataMayBeStale: true,
        },
      },
    };
    await installTauriMock(page, rateLimitedSummary);
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    const chip = page.locator(".panel-codex .chip");
    await chip.waitFor();
    assert.equal(await chip.getAttribute("class"), "chip chip-warn");
    assert.equal(await chip.locator(".chip-tag").textContent(), "LIMIT");
    assert.equal(await chip.locator(".chip-text").textContent(), "RATE LIMITED");
    assert.equal(await chip.getAttribute("aria-label"), "LIMIT: RATE LIMITED");
    await context.close();
  }
}

async function checkMascotMoods(browser) {
  const cases = [
    {
      name: "idle",
      mood: "idle",
      services: {
        ...baseSummary.services,
        claude: { ...baseSummary.services.claude, fiveHourPercent: 12, sevenDayPercent: 20 },
      },
    },
    {
      name: "focus",
      mood: "focus",
      services: {
        ...baseSummary.services,
        claude: { ...baseSummary.services.claude, fiveHourPercent: 42, sevenDayPercent: 55 },
      },
    },
    {
      name: "tense",
      mood: "tense",
      services: {
        ...baseSummary.services,
        claude: { ...baseSummary.services.claude, fiveHourPercent: 71, sevenDayPercent: 80 },
      },
    },
    {
      name: "over",
      mood: "over",
      services: {
        ...baseSummary.services,
        claude: { ...baseSummary.services.claude, fiveHourPercent: 91, sevenDayPercent: 88 },
      },
    },
    {
      name: "error",
      mood: "error",
      services: {
        ...baseSummary.services,
        claude: { ...baseSummary.services.claude, status: "LOGIN REQUIRED" },
      },
    },
  ];

  for (const testCase of cases) {
    const context = await browser.newContext({ viewport: { width: 1368, height: 912 } });
    const page = await context.newPage();
    await installTauriMock(page, {
      ...baseSummary,
      services: testCase.services,
    });
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    const mascot = page.locator(".panel-claude .panel-mascot");
    await mascot.waitFor();
    assert.equal(await mascot.getAttribute("data-mascot"), testCase.mood, testCase.name);
    const src = await mascot.getAttribute("src");
    assert.match(src ?? "", /claude-/);
    if (testCase.name === "over") {
      assert.equal(await page.locator(".panel-claude .gauge-stage").getAttribute("data-tone"), "danger");
      assert.equal(
        await page.locator(".panel-claude .gauge-readout").evaluate((el) => el.classList.contains("is-pulse")),
        true,
      );
      assert.match((await page.locator(".panel-claude .gauge-readout").textContent()) ?? "", /91/);
    }
    if (testCase.name === "tense") {
      assert.equal(await page.locator(".panel-claude .gauge-stage").getAttribute("data-tone"), "warn");
    }
    assert.deepEqual(await inspectLayout(page, { width: 1368, height: 912 }), []);
    await context.close();
  }

  const deepseekContext = await browser.newContext({ viewport: { width: 1368, height: 912 } });
  const deepseekPage = await deepseekContext.newPage();
  await installTauriMock(deepseekPage, baseSummary);
  await deepseekPage.goto(baseUrl, { waitUntil: "networkidle" });
  assert.equal(
    await deepseekPage.locator(".panel-deepseek .panel-mascot").getAttribute("data-mascot"),
    "idle",
  );
  await deepseekContext.close();
}

async function checkUsageVariants(browser) {
  const dualWindowCodex = baseSummary.services.codex;
  const weeklyOnlyCodex = {
    status: "NOMINAL",
    fromCache: false,
    dataMayBeStale: false,
    plan: "pro",
    sevenDayPercent: 61,
    sevenDayResetLocal: "07-29 01:02",
    resetCreditsAvailable: 3,
    resetCreditsExpireLocal: "08-15 12:00",
  };
  const dualWindowClaude = baseSummary.services.claude;

  const variants = [
    {
      name: "codex-weekly-only",
      summary: {
        ...baseSummary,
        services: {
          ...baseSummary.services,
          codex: weeklyOnlyCodex,
        },
      },
      codexLabels: ["7D"],
      claudeLabels: ["7D", "5H"],
    },
    {
      name: "claude-weekly-only",
      summary: {
        ...baseSummary,
        services: {
          ...baseSummary.services,
          codex: dualWindowCodex,
          claude: {
            status: "NOMINAL",
            fromCache: false,
            dataMayBeStale: false,
            sevenDayPercent: 28,
            sevenDayResetLocal: "07-29 09:30",
          },
        },
      },
      codexLabels: ["7D", "5H"],
      claudeLabels: ["7D"],
    },
    {
      name: "claude-extra-only",
      summary: {
        ...baseSummary,
        services: {
          ...baseSummary.services,
          codex: dualWindowCodex,
          claude: {
            status: "NOMINAL",
            fromCache: false,
            dataMayBeStale: false,
            extraUsagePercent: 12.5,
          },
        },
      },
      codexLabels: ["7D", "5H"],
      claudeLabels: ["EXTRA"],
    },
    {
      name: "codex-dual-window",
      summary: {
        ...baseSummary,
        services: {
          ...baseSummary.services,
          codex: dualWindowCodex,
          claude: dualWindowClaude,
        },
      },
      codexLabels: ["7D", "5H"],
      claudeLabels: ["7D", "5H"],
    },
  ];

  for (const variant of variants) {
    const viewport = { width: 1368, height: 912 };
    const context = await browser.newContext({
      viewport,
      hasTouch: true,
      reducedMotion: "reduce",
      colorScheme: "dark",
    });
    const page = await context.newPage();
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));
    await installTauriMock(page, variant.summary);
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.locator(".panel").first().waitFor();

    assert.deepEqual(await legendWindows(page, ".panel-codex"), variant.codexLabels);
    assert.equal(await page.locator(".panel-codex .reset-credits").count(), 1);
    await page.locator(".panel-codex .reset-credits", { hasText: "3 AVAILABLE" }).waitFor();
    await page.locator(".panel-codex .reset-credits", { hasText: "FIRST EXP" }).waitFor();
    assert.deepEqual(await legendWindows(page, ".panel-claude"), variant.claudeLabels);
    assert.deepEqual(await legendWindows(page, ".panel-grok"), ["7D", "MONTH"]);
    assert.deepEqual(await inspectLayout(page, viewport), []);
    assert.deepEqual(pageErrors, [], `${variant.name} page errors`);
    await page.screenshot({ path: join(artifactDir, `${variant.name}.png`), fullPage: true });
    await context.close();
  }
}

async function checkLowPowerMode(browser) {
  const context = await browser.newContext({
    viewport: { width: 1368, height: 912 },
    reducedMotion: "no-preference",
    colorScheme: "dark",
  });
  const page = await context.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await installTauriMock(page, summaries.normal);
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.locator('.dashboard[data-low-power="false"]').waitFor();
  await page.locator(".gauge-stage[data-mood='focus'] .panel-mascot").first().waitFor();

  assert.match(
    await page
      .locator(".gauge-stage[data-mood='focus'] .panel-mascot")
      .first()
      .evaluate((element) => getComputedStyle(element).animationName),
    /^mascot-float/,
  );
  assert.equal(
    await page.locator(".scanlines").evaluate((element) => getComputedStyle(element).display),
    "none",
  );
  assert.equal(await page.locator(".clock").getAttribute("data-cadence"), "second");
  assert.match((await page.locator(".clock").textContent()) ?? "", /^\d{2}:\d{2}:\d{2}$/);

  await page.locator(".tm-settings").click();
  const toggle = page.locator('input[name="lowPowerMode"]');
  assert.equal(await toggle.isChecked(), false);
  await page.locator(".performance-option").click();
  await page.locator(".settings-save").click();
  await page.locator('.dashboard[data-low-power="true"]').waitFor();

  assert.equal(
    await page
      .locator(".gauge-stage[data-mood='focus'] .panel-mascot")
      .first()
      .evaluate((element) => getComputedStyle(element).animationName),
    "none",
  );
  assert.equal(
    await page.locator(".scanlines").evaluate((element) => getComputedStyle(element).display),
    "none",
  );
  assert.equal(await page.locator(".gauge-stage").count(), 4);
  assert.equal(await page.locator(".clock").getAttribute("data-cadence"), "minute");
  assert.match((await page.locator(".clock").textContent()) ?? "", /^\d{2}:\d{2}$/);
  assert.equal(
    await page.evaluate(() =>
      document.getAnimations().filter((animation) => animation.playState === "running").length,
    ),
    0,
  );
  assert.deepEqual(await inspectLayout(page, { width: 1368, height: 912 }), []);
  assert.deepEqual(pageErrors, [], "low-power page errors");
  assert.deepEqual(await page.evaluate(() => window.__DASHBOARD_TEST__.saveCalls[0]), {
    enabledProviders: null,
    windowMode: null,
    lowPowerMode: true,
  });

  await page.locator(".tm-settings").click();
  assert.equal(await toggle.isChecked(), true);
  await page.locator(".performance-option").click();
  await page.locator(".settings-save").click();
  await page.locator('.dashboard[data-low-power="false"]').waitFor();
  assert.equal(await page.locator(".clock").getAttribute("data-cadence"), "second");
  assert.deepEqual(await page.evaluate(() => window.__DASHBOARD_TEST__.saveCalls[1]), {
    enabledProviders: null,
    windowMode: null,
    lowPowerMode: false,
  });
  await context.close();

  const persistedContext = await browser.newContext({
    viewport: { width: 1368, height: 912 },
    reducedMotion: "no-preference",
    colorScheme: "dark",
  });
  const persistedPage = await persistedContext.newPage();
  await installTauriMock(persistedPage, summaries.normal, "normal", false, true);
  await persistedPage.goto(baseUrl, { waitUntil: "networkidle" });
  await persistedPage.locator('.dashboard[data-low-power="true"]').waitFor();
  await persistedPage.locator(".tm-settings").click();
  assert.equal(await persistedPage.locator('input[name="lowPowerMode"]').isChecked(), true);
  await persistedContext.close();

  for (const viewport of [
    { width: 640, height: 540 },
    { width: 684, height: 912 },
  ]) {
    const compactContext = await browser.newContext({
      viewport,
      hasTouch: true,
      reducedMotion: "no-preference",
      colorScheme: "dark",
    });
    const compactPage = await compactContext.newPage();
    const compactErrors = [];
    compactPage.on("pageerror", (error) => compactErrors.push(error.message));
    await installTauriMock(compactPage, summaries.normal, "normal", false, true);
    await compactPage.goto(baseUrl, { waitUntil: "networkidle" });
    await compactPage.locator('.dashboard[data-low-power="true"]').waitFor();
    assert.deepEqual(await inspectLayout(compactPage, viewport), []);
    await compactPage.locator(".tm-settings").click();
    const dialog = compactPage.locator(".settings-dialog");
    await dialog.waitFor();
    const dialogBox = await dialog.boundingBox();
    assert.ok(dialogBox, `${viewport.width}x${viewport.height} settings dialog missing`);
    assert.ok(dialogBox.x >= 0 && dialogBox.x + dialogBox.width <= viewport.width + 1);
    assert.ok(dialogBox.y >= 0 && dialogBox.y + dialogBox.height <= viewport.height + 1);
    assert.equal(await compactPage.locator(".provider-grid .provider-option").count(), 6);
    await compactPage.locator(".provider-option.provider-cursor").scrollIntoViewIfNeeded();
    await compactPage.locator(".provider-option.provider-antigravity").scrollIntoViewIfNeeded();
    assert.equal(await compactPage.locator(".provider-option.provider-cursor").isVisible(), true);
    assert.equal(
      await compactPage.locator(".provider-option.provider-antigravity").isVisible(),
      true,
    );
    await compactPage.locator(".settings-save").scrollIntoViewIfNeeded();
    assert.equal(await compactPage.locator(".settings-save").isVisible(), true);
    assert.deepEqual(compactErrors, [], `${viewport.width}x${viewport.height} low-power errors`);
    await compactContext.close();
  }
}

async function checkIpPlanCards(browser) {
  const viewport = { width: 1600, height: 900 };
  const five = {
    ...baseSummary,
    enabledProviders: {
      ...baseSummary.enabledProviders,
      cursor: true,
    },
  };

  async function open(summary, size = viewport) {
    const context = await browser.newContext({
      viewport: size,
      reducedMotion: "reduce",
      colorScheme: "dark",
    });
    const page = await context.newPage();
    await installTauriMock(page, summary);
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.locator(".panel").first().waitFor();
    await page.locator('.dashboard[data-art="ip"]').waitFor();
    return { context, page };
  }

  {
    const { context, page } = await open(five);
    assert.equal(await page.locator(".panels").getAttribute("data-layout"), "five-split");
    assert.equal(await page.locator(".panel-grok").getAttribute("data-meter"), "bar");
    assert.equal(await page.locator(".panel-cursor").getAttribute("data-meter"), "bar");
    assert.equal(await page.locator(".panel-codex").getAttribute("data-meter"), "ring");
    assert.equal(await page.locator(".panel-claude").getAttribute("data-meter"), "ring");
    assert.equal(await page.locator(".panel-deepseek").getAttribute("data-meter"), "ring");
    await assertPlanPill(page, ".panel-grok", "SUPERGROK HEAVY");
    await assertPlanPill(page, ".panel-cursor", "ULTRA");
    await assertPlanPill(page, ".panel-codex", "PRO");
    assert.match(
      (await page.locator(".panel-grok .reset-credits").textContent()) ?? "",
      /BANKED RESETS/,
    );
    await assertIpTitles(page);
    assert.deepEqual(await inspectLayout(page, viewport), []);
    await page.screenshot({ path: join(artifactDir, "ip-plan-five-split.png"), fullPage: true });
    await context.close();
  }

  {
    const summary = {
      ...baseSummary,
      enabledProviders: {
        codex: true,
        claude: true,
        deepseek: false,
        grok: true,
        cursor: true,
        antigravity: true,
      },
    };
    const { context, page } = await open(summary);
    assert.equal(await page.locator(".panels").getAttribute("data-layout"), "five-split");
    assert.equal(await page.locator(".panel-antigravity").getAttribute("data-meter"), "ring");
    assert.equal(await page.locator(".panel-antigravity").getAttribute("data-plan-size"), "M");
    await assertPlanPill(page, ".panel-antigravity", "GOOGLE AI PRO");
    assert.match(
      (await page.locator(".panel-antigravity .gauge-readout").textContent()) ?? "",
      /21/,
    );
    assert.doesNotMatch(
      (await page.locator(".panel-antigravity").textContent()) ?? "",
      /\bFREE\b/,
    );
    await page.screenshot({
      path: join(artifactDir, "ip-plan-ag-compact-ring.png"),
      fullPage: true,
    });
    await context.close();
  }

  {
    const summary = {
      ...baseSummary,
      enabledProviders: {
        codex: true,
        claude: true,
        deepseek: false,
        grok: true,
        cursor: true,
        antigravity: true,
      },
      services: {
        ...baseSummary.services,
        antigravity: { ...baseSummary.services.antigravity, plan: "Antigravity" },
      },
    };
    const { context, page } = await open(summary);
    assert.equal(await page.locator(".panel-antigravity .panel-plan").count(), 0);
    assert.doesNotMatch(
      (await page.locator(".panel-antigravity").textContent()) ?? "",
      /\bFREE\b/,
    );
    await context.close();
  }

  {
    const summary = {
      ...five,
      services: {
        ...five.services,
        cursor: { ...five.services.cursor, plan: "Pro" },
      },
    };
    const { context, page } = await open(summary);
    assert.equal(await page.locator(".panels").getAttribute("data-layout"), "five-hero");
    assert.equal(await page.locator(".panel-grok").getAttribute("data-meter"), "bar");
    assert.equal(await page.locator(".panel-cursor").getAttribute("data-meter"), "ring");
    assert.equal(await page.locator(".panel-cursor").getAttribute("data-plan-size"), "M");
    await assertPlanPill(page, ".panel-grok", "SUPERGROK HEAVY");
    await assertPlanPill(page, ".panel-cursor", "PRO");
    await assertIpTitles(page);
    assert.deepEqual(await inspectLayout(page, viewport), []);
    await page.screenshot({ path: join(artifactDir, "ip-plan-five-hero.png"), fullPage: true });
    await context.close();
  }

  {
    const summary = {
      ...five,
      services: {
        ...five.services,
        grok: { ...five.services.grok, plan: "SuperGrok" },
        cursor: { ...five.services.cursor, plan: "Pro" },
      },
    };
    const { context, page } = await open(summary);
    assert.equal(await page.locator(".panels").getAttribute("data-layout"), "five-fill");
    await assertPlanPill(page, ".panel-grok", "SUPERGROK");
    await assertPlanPill(page, ".panel-cursor", "PRO");
    await assertIpTitles(page);
    assert.deepEqual(await inspectLayout(page, viewport), []);
    await page.screenshot({ path: join(artifactDir, "ip-plan-five-fill.png"), fullPage: true });
    await context.close();
  }

  {
    const summary = {
      ...baseSummary,
      enabledProviders: {
        codex: true,
        claude: false,
        deepseek: true,
        grok: true,
        cursor: true,
        antigravity: true,
      },
      services: {
        ...baseSummary.services,
        grok: { ...baseSummary.services.grok, plan: "SuperGrok" },
        cursor: { ...baseSummary.services.cursor, plan: "Pro" },
        antigravity: { ...baseSummary.services.antigravity, plan: "Google AI Ultra" },
      },
    };
    const { context, page } = await open(summary);
    assert.equal(await page.locator(".panels").getAttribute("data-layout"), "five-hero");
    assert.equal(await page.locator(".panel-antigravity").getAttribute("data-slot"), "hero");
    assert.equal(await page.locator(".panel-antigravity").getAttribute("data-meter"), "bar");
    assert.equal(await page.locator(".panel-grok").getAttribute("data-meter"), "ring");
    await assertPlanPill(page, ".panel-antigravity", "GOOGLE AI ULTRA");
    await assertPlanPill(page, ".panel-grok", "SUPERGROK");
    await assertPlanPill(page, ".panel-cursor", "PRO");
    await assertIpTitles(page);
    assert.deepEqual(await inspectLayout(page, viewport), []);
    await page.screenshot({ path: join(artifactDir, "ip-plan-ag-ultra-hero.png"), fullPage: true });
    await context.close();
  }

  {
    const summary = {
      ...five,
      services: {
        ...five.services,
        grok: { ...five.services.grok, plan: "Free" },
        cursor: { ...five.services.cursor, plan: "Hobby" },
      },
    };
    const { context, page } = await open(summary);
    assert.equal(await page.locator(".panels").getAttribute("data-layout"), "five-fill");
    assert.equal(await page.locator(".panel-grok").getAttribute("data-plan-size"), "S");
    assert.equal(await page.locator(".panel-cursor").getAttribute("data-plan-size"), "S");
    await assertPlanPill(page, ".panel-grok", "FREE");
    await assertPlanPill(page, ".panel-cursor", "HOBBY");
    await assertIpTitles(page);
    assert.deepEqual(await inspectLayout(page, viewport), []);
    await context.close();
  }

  {
    const summary = {
      ...five,
      services: {
        ...five.services,
        cursor: { ...five.services.cursor, plan: "Enterprise" },
      },
    };
    const { context, page } = await open(summary);
    assert.equal(await page.locator(".panels").getAttribute("data-layout"), "five-split");
    assert.equal(await page.locator(".panel-cursor").getAttribute("data-meter"), "bar");
    await assertPlanPill(page, ".panel-cursor", "ENTERPRISE");
    await assertPlanPill(page, ".panel-grok", "SUPERGROK HEAVY");
    assert.deepEqual(await inspectLayout(page, viewport), []);
    await context.close();
  }

  {
    const compact = { width: 960, height: 540 };
    const { context, page } = await open(five, compact);
    assert.equal(await page.locator(".panels").getAttribute("data-layout"), "five-split");
    await assertPlanPill(page, ".panel-grok", "SUPERGROK HEAVY");
    await assertPlanPill(page, ".panel-cursor", "ULTRA");
    await assertIpTitles(page);
    assert.deepEqual(await inspectLayout(page, compact), []);
    await page.screenshot({ path: join(artifactDir, "ip-plan-five-split-compact.png"), fullPage: true });
    await context.close();
  }
}

async function checkArtSkin(browser) {
  const context = await browser.newContext({
    viewport: { width: 1368, height: 912 },
    reducedMotion: "no-preference",
    colorScheme: "dark",
  });
  const page = await context.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  await installTauriMock(page, summaries.normal);
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.locator('.dashboard[data-art="ip"]').waitFor();
  assert.equal(await page.locator(".panel-mascot").first().getAttribute("data-pack"), "ip");
  assert.equal(await page.locator(".panel[data-emerge]").count(), await page.locator(".panel").count());
  assert.equal(await page.locator(".rail").evaluate((el) => getComputedStyle(el).display), "none");
  const ipFill = await page.evaluate(() => {
    const panels = document.querySelector(".panels");
    const dash = document.querySelector(".dashboard");
    if (!panels || !dash) return { w: 0, h: 0 };
    const panelRect = panels.getBoundingClientRect();
    const dashRect = dash.getBoundingClientRect();
    return {
      w: panelRect.width / Math.max(dashRect.width, 1),
      h: panelRect.height / Math.max(dashRect.height, 1),
    };
  });
  assert.ok(ipFill.w >= 0.9, `IP panels should fill width, got ${ipFill.w.toFixed(3)}`);
  assert.ok(ipFill.h >= 0.84, `IP panels should fill height, got ${ipFill.h.toFixed(3)}`);
  assert.deepEqual(await inspectLayout(page, { width: 1368, height: 912 }), []);

  await page.locator(".tm-settings").click();
  assert.equal(await page.locator('input[name="artSkin"][value="ip"]').isChecked(), true);
  await page
    .locator('label.window-mode-option:has(input[name="artSkin"][value="ring"])')
    .click();
  await page.locator(".settings-save").click();
  await page.locator('.dashboard[data-art="ring"]').waitFor();
  assert.equal(
    await page.locator(".panel-deepseek .ip-bars").evaluate((el) => getComputedStyle(el).alignItems),
    "center",
  );
  assert.equal(
    await page.locator(".panel-deepseek .ip-bars-peak").evaluate((el) => getComputedStyle(el).textAlign),
    "center",
  );
  assert.equal(await page.locator(".panel-mascot").count(), 0);
  assert.ok((await page.locator(".ip-bars").count()) >= 1);
  assert.equal(await page.locator(".gauge-stage").count(), 0);
  assert.deepEqual(await page.evaluate(() => window.__DASHBOARD_TEST__.saveCalls[0]), {
    enabledProviders: null,
    windowMode: null,
    artSkin: "ring",
  });
  assert.deepEqual(pageErrors, [], "art-skin page errors");
  await page.screenshot({ path: join(artifactDir, "art-skin-ring.png"), fullPage: true });
  await context.close();

  const fiveSplit = {
    ...summaries.normal,
    enabledProviders: {
      ...summaries.normal.enabledProviders,
      cursor: true,
    },
  };
  const splitContext = await browser.newContext({
    viewport: { width: 1368, height: 912 },
    reducedMotion: "no-preference",
    colorScheme: "dark",
  });
  const splitPage = await splitContext.newPage();
  const splitErrors = [];
  splitPage.on("pageerror", (error) => splitErrors.push(error.message));
  await installTauriMock(splitPage, fiveSplit, "normal", false, false, "ip");
  await splitPage.goto(baseUrl, { waitUntil: "networkidle" });
  await splitPage.locator('.dashboard[data-art="ip"]').waitFor();
  assert.equal(await splitPage.locator(".panels").getAttribute("data-layout"), "five-split");
  await splitPage.locator('.panel-grok[data-meter="bar"]').waitFor();
  await splitPage.locator('.panel-cursor[data-meter="bar"]').waitFor();
  assert.equal(await splitPage.locator(".panel-codex").getAttribute("data-meter"), "ring");
  assert.equal(await splitPage.locator(".panel-claude").getAttribute("data-meter"), "ring");
  assert.equal(await splitPage.locator(".panel-deepseek").getAttribute("data-meter"), "ring");
  const grokColumns = await splitPage.locator(".panel-grok .panel-body").evaluate(
    (element) => getComputedStyle(element).gridTemplateColumns.split(/\s+/).filter(Boolean).length,
  );
  assert.ok(grokColumns >= 2, `wide Grok body should be two columns, got ${grokColumns}`);
  assert.notEqual(
    await splitPage.locator(".panel-grok .ip-bars").evaluate((element) => getComputedStyle(element).display),
    "none",
  );
  assert.equal(await splitPage.locator(".panel-grok .gauge-stage").count(), 0);
  assert.equal(await splitPage.locator(".panel-codex .gauge-stage").count(), 1);
  const grokWindows = await splitPage
    .locator(".panel-grok .ip-bar")
    .evaluateAll((els) => els.map((el) => el.getAttribute("data-window")).filter(Boolean));
  assert.deepEqual(grokWindows, ["7D", "MONTH"]);
  const grokMascot = await splitPage.locator(".panel-grok .panel-mascot").boundingBox();
  assert.ok(
    grokMascot && grokMascot.height >= 140,
    `Grok mascot should fill the stage, got ${grokMascot?.height ?? 0}px`,
  );
  assert.equal(await splitPage.locator(".panel-grok").getAttribute("data-emerge"), "left");
  const grokBars = await splitPage.locator(".panel-grok .ip-bars").boundingBox();
  assert.ok(
    grokMascot && grokBars && grokBars.x > grokMascot.x + 40,
    `Grok should sit on the left of the bars, mascot.x=${grokMascot?.x ?? 0} bars.x=${grokBars?.x ?? 0}`,
  );
  const grokFlip = await splitPage
    .locator(".panel-grok .ip-portrait")
    .evaluate((element) => getComputedStyle(element).transform);
  assert.doesNotMatch(
    grokFlip,
    /matrix3?d?\(-1/,
    `Grok portrait should face the usage bars unflipped, got ${grokFlip}`,
  );
  assert.match(
    (await splitPage.locator(".panel-grok .ip-bar-reset").first().textContent()) ?? "",
    /RESET/,
  );
  assert.equal(await splitPage.locator(".panel-claude").getAttribute("data-tone"), "warn");
  const claudeReadout = await splitPage.locator(".panel-claude .gauge-readout").evaluate((el) => {
    const style = getComputedStyle(el);
    return { color: style.color, bg: style.backgroundColor };
  });
  assert.notEqual(claudeReadout.bg, "rgba(0, 0, 0, 0)", "ring peak should sit on an ink chip");
  assert.match(
    (await splitPage.locator(".panel-codex .gauge-legend").textContent()) ?? "",
    /7D 61/,
  );
  assert.match(
    (await splitPage.locator(".panel-codex .gauge-legend").textContent()) ?? "",
    /07-29 01:02/,
  );
  assert.doesNotMatch(
    (await splitPage.locator(".panel-codex .gauge-legend").textContent()) ?? "",
    /RESET/,
  );
  const titleColors = await splitPage.locator(".panel-title").evaluateAll((els) =>
    els.map((el) => getComputedStyle(el).color),
  );
  assert.ok(titleColors.length >= 5, `expected five IP titles, got ${titleColors.length}`);
  assert.ok(
    titleColors.every((color) => color === titleColors[0]),
    `IP titles should share one color, got ${titleColors.join(" | ")}`,
  );
  assert.equal(titleColors[0], "rgb(255, 248, 240)");
  const grokPlan = await splitPage.locator(".panel-grok .panel-plan").evaluate((el) => {
    const style = getComputedStyle(el);
    return {
      text: (el.textContent ?? "").trim(),
      whiteSpace: style.whiteSpace,
      lines: el.getClientRects().length,
    };
  });
  assert.equal(grokPlan.text, "SUPERGROK HEAVY");
  assert.equal(grokPlan.whiteSpace, "nowrap");
  assert.equal(grokPlan.lines, 1, "SUPERGROK HEAVY should hug one line without leftover black");
  const clockBox = await splitPage.locator(".clockwrap").boundingBox();
  const settingsBox = await splitPage.locator(".tm-settings").boundingBox();
  assert.ok(clockBox && settingsBox, "clock and settings should render");
  const chromeGap = settingsBox.x - (clockBox.x + clockBox.width);
  assert.ok(
    chromeGap >= 24,
    `clock and settings should stay apart, gap=${chromeGap.toFixed(1)}`,
  );
  const ringFit = await splitPage.locator(".panel-codex .gauge-stage").evaluate((stage) => {
    const figure = stage.querySelector(".gauge-figure");
    if (!figure) return null;
    const stageRect = stage.getBoundingClientRect();
    const figureRect = figure.getBoundingClientRect();
    const stageSize = Math.min(stageRect.width, stageRect.height);
    const inset = Math.min(
      figureRect.left - stageRect.left,
      figureRect.top - stageRect.top,
      stageRect.right - figureRect.right,
      stageRect.bottom - figureRect.bottom,
    );
    return {
      ratio: Math.min(figureRect.width, figureRect.height) / stageSize,
      inset,
      radius: getComputedStyle(figure).borderRadius,
    };
  });
  assert.ok(ringFit && ringFit.ratio <= 0.7, `ring mascot should sit inside the hole, ratio=${ringFit?.ratio}`);
  assert.ok(ringFit && ringFit.inset >= 12, `ring mascot should clear the ring stroke, inset=${ringFit?.inset}`);
  assert.equal(ringFit?.radius, "50%");
  assert.deepEqual(await inspectLayout(splitPage, { width: 1368, height: 912 }), []);
  assert.deepEqual(splitErrors, [], "IP five-split page errors");
  await splitPage.screenshot({ path: join(artifactDir, "art-skin-ip-five-split.png"), fullPage: true });
  await splitContext.close();

  const persistedContext = await browser.newContext({
    viewport: { width: 1368, height: 912 },
    reducedMotion: "no-preference",
    colorScheme: "dark",
  });
  const persistedPage = await persistedContext.newPage();
  await installTauriMock(persistedPage, summaries.normal, "normal", false, false, "ip");
  await persistedPage.goto(baseUrl, { waitUntil: "networkidle" });
  await persistedPage.locator('.dashboard[data-art="ip"]').waitFor();
  await persistedPage.locator(".tm-settings").click();
  assert.equal(await persistedPage.locator('input[name="artSkin"][value="ip"]').isChecked(), true);
  await persistedContext.close();
}

async function checkJudgeDemo(browser) {
  const judgeViewports = [
    { name: "surface", width: 1368, height: 912 },
    { name: "compact", width: 960, height: 540 },
    { name: "snap", width: 684, height: 912 },
  ];

  for (const viewport of judgeViewports) {
    const context = await browser.newContext({
      viewport,
      hasTouch: true,
      reducedMotion: "reduce",
      colorScheme: "dark",
    });
    const page = await context.newPage();
    await installTauriMock(page, summaries.normal, "normal", true);
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.locator(".tm-demo", { hasText: "SYNTHETIC DEMO" }).waitFor();
    assert.equal(await page.locator(".panel").count(), 4);
    assert.deepEqual(await inspectLayout(page, viewport), []);

    if (viewport.name === "surface") {
      await page.locator(".tm-settings").click();
      assert.equal(await page.locator("#deepseek-api-key").isDisabled(), true);
      assert.equal(await page.locator("#deepseek-api-key").inputValue(), "");
      await page.keyboard.press("Escape");
      await page.locator(".settings-dialog").waitFor({ state: "detached" });
    }

    const refresh = page.locator(".tm-refresh");
    await refresh.click();
    await page.evaluate(() => window.__DASHBOARD_TEST__.completeRefresh());
    await page.locator(".tm-refresh:not(:disabled)").waitFor();

    if (viewport.name === "surface") {
      await page.screenshot({ path: join(artifactDir, "judge-demo.png"), fullPage: true });
    }
    await context.close();
  }
}

async function captureReadmeScreenshots(browser) {
  const twoStacked = {
    ...baseSummary,
    enabledProviders: {
      codex: false,
      claude: false,
      deepseek: false,
      grok: true,
      cursor: true,
      antigravity: false,
    },
  };
  const allSix = {
    ...baseSummary,
    enabledProviders: {
      codex: true,
      claude: true,
      deepseek: true,
      grok: true,
      cursor: true,
      antigravity: true,
    },
  };

  async function openShot(summary, { width, height, skin }) {
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor: 2,
      reducedMotion: "reduce",
      colorScheme: "dark",
    });
    const page = await context.newPage();
    await installTauriMock(page, summary, "normal", false, false, skin);
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.locator(".panel").first().waitFor();
    await page.locator(`.dashboard[data-art="${skin}"]`).waitFor();
    return { context, page };
  }

  await mkdir(readmeAssetDir, { recursive: true });

  {
    const { context, page } = await openShot(twoStacked, { width: 1600, height: 900, skin: "ip" });
    assert.equal(await page.locator(".panel").count(), 2);
    assert.equal(await page.locator(".panel-grok").getAttribute("data-meter"), "bar");
    assert.equal(await page.locator(".panel-cursor").getAttribute("data-meter"), "bar");
    const stacked = await page.locator(".panels").evaluate((el) => {
      const grok = el.querySelector(".panel-grok")?.getBoundingClientRect();
      const cursor = el.querySelector(".panel-cursor")?.getBoundingClientRect();
      if (!grok || !cursor) return false;
      return cursor.top >= grok.bottom - 2 && Math.abs(grok.left - cursor.left) < 2;
    });
    assert.equal(stacked, true, "two IP cards stack as landscape rows");
    assert.deepEqual(await inspectLayout(page, { width: 1600, height: 900 }), []);
    await page.screenshot({ path: join(readmeAssetDir, "dashboard.png") });
    await context.close();
  }

  {
    const { context, page } = await openShot(allSix, { width: 1600, height: 1000, skin: "ip" });
    assert.equal(await page.locator(".panel").count(), 6);
    const meters = await page.locator(".panel").evaluateAll((els) =>
      els.map((el) => el.getAttribute("data-meter")),
    );
    assert.equal(meters.length, 6);
    assert.ok(
      meters.every((meter) => meter === "ring"),
      `expected six IP rings, got ${meters.join(",")}`,
    );
    assert.deepEqual(await inspectLayout(page, { width: 1600, height: 1000 }), []);
    await page.screenshot({ path: join(readmeAssetDir, "dashboard-six-rings.png") });
    await context.close();
  }

  {
    const { context, page } = await openShot(allSix, { width: 1600, height: 1000, skin: "ring" });
    assert.equal(await page.locator(".panel").count(), 6);
    assert.equal(await page.locator(".panel-mascot").count(), 0);
    assert.ok((await page.locator(".ip-bars").count()) >= 6);
    assert.deepEqual(await inspectLayout(page, { width: 1600, height: 1000 }), []);
    await page.screenshot({ path: join(readmeAssetDir, "dashboard-eva.png") });
    await context.close();
  }

  const mascotDir = join(readmeAssetDir, "mascots-ip");
  await mkdir(mascotDir, { recursive: true });
  for (const kind of ["codex", "claude", "deepseek", "grok", "cursor", "antigravity"]) {
    await copyFile(
      join(root, "src", "assets", "mascots-ip", `${kind}-idle.png`),
      join(mascotDir, `${kind}-idle.png`),
    );
  }
}

async function launchTestBrowser() {
  try {
    return await chromium.launch({ headless: true });
  } catch (error) {
    if (process.platform === "win32") {
      return chromium.launch({ channel: "msedge", headless: true });
    }
    if (process.platform === "darwin") {
      return chromium.launch({ channel: "chrome", headless: true });
    }
    throw error;
  }
}

await mkdir(artifactDir, { recursive: true });
const server = startServer();
let browser;

try {
  await waitForServer(server);
  browser = await launchTestBrowser();

  for (const [stateName, summary] of Object.entries(summaries)) {
    for (const viewport of viewports) {
      await checkScenario(browser, stateName, summary, viewport);
      process.stdout.write(`PASS ${stateName.padEnd(10)} ${viewport.width}x${viewport.height}\n`);
    }
  }
  await checkProviderSelection(browser);
  process.stdout.write(`PASS provider selection and 0/1/2/3/4-panel layouts\n`);
  await checkCursorPanel(browser);
  process.stdout.write(`PASS Cursor panel meters and 5-panel layout\n`);
  await checkAntigravityPanel(browser);
  process.stdout.write(`PASS Antigravity Gemini meters and 6-panel layout\n`);
  await checkMascotMoods(browser);
  process.stdout.write(`PASS Q-version mascot moods follow usage and auth\n`);
  await checkSettingsTransactions(browser);
  process.stdout.write(`PASS transactional settings, CLI override, focus, and save failures\n`);
  await checkDeepSeekKeySettings(browser);
  process.stdout.write(`PASS direct DeepSeek key settings and redacted test telemetry\n`);
  await checkAuthStatusChips(browser);
  process.stdout.write(`PASS provider auth-status chip mapping\n`);
  await checkRateLimitStatusChips(browser);
  process.stdout.write(`PASS rate-limit status overrides stale freshness\n`);
  await checkFullscreenThreeColumns(browser);
  process.stdout.write(`PASS fullscreen three-column layout across Surface/Full HD sizes\n`);
  await checkUsageVariants(browser);
  process.stdout.write(`PASS dynamic Codex and Claude usage-window variants\n`);
  await checkLowPowerMode(browser);
  process.stdout.write(`PASS low-power motion, compositing, clock, and persistence behavior\n`);
  await checkArtSkin(browser);
  process.stdout.write(`PASS art-skin ring/ip toggle, mascot pack, and persistence\n`);
  await checkIpPlanCards(browser);
  process.stdout.write(`PASS IP large/small cards across subscription plans\n`);
  await checkJudgeDemo(browser);
  process.stdout.write(`PASS isolated judge demo across Surface/compact/Snap layouts\n`);
  await checkKeyboardAndScreensaver(browser);
  process.stdout.write(`PASS keyboard and screensaver input\n`);
  await captureReadmeScreenshots(browser);
  process.stdout.write(`PASS README synthetic screenshots\nScreenshots: ${artifactDir}\n`);
} finally {
  await browser?.close();
  if (server.exitCode === null) server.kill("SIGTERM");
}
