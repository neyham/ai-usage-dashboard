import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const port = Number(process.env.UI_TEST_PORT ?? 1432);
const baseUrl = `http://127.0.0.1:${port}`;
const artifactDir =
  process.env.UI_ARTIFACT_DIR ?? join(tmpdir(), "ai-usage-dashboard-surface-results");

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
  enabledProviders: { codex: true, claude: true, deepseek: true, grok: true },
  services: {
    codex: {
      status: "NOMINAL",
      fromCache: false,
      dataMayBeStale: false,
      plan: "pro",
      sevenDayPercent: 61,
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

async function installTauriMock(page, summary, launchMode = "normal", judgeDemo = false) {
  await page.addInitScript(
    ({ initialSummary, mode, demo }) => {
      let currentSummary = structuredClone(initialSummary);
      let nextCallbackId = 1;
      let nextEventId = 1;
      const callbacks = new Map();
      const listeners = new Map();
      let refreshPending = false;
      const stats = { exits: 0, refreshes: 0, savedSelections: [], completeRefresh() {} };

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
          if (command === "judge_demo") return demo;
          if (command === "refresh_now") {
            stats.refreshes += 1;
            refreshPending = true;
            return true;
          }
          if (command === "save_enabled_providers") {
            currentSummary = {
              ...currentSummary,
              enabledProviders: structuredClone(args.enabledProviders),
            };
            stats.savedSelections.push(structuredClone(args.enabledProviders));
            emit("summary", structuredClone(currentSummary));
            return structuredClone(args.enabledProviders);
          }
          if (command === "exit_app") {
            stats.exits += 1;
            return null;
          }
          throw new Error(`Unexpected Tauri command: ${command}`);
        },
      };
      window.__DASHBOARD_TEST__ = stats;
    },
    { initialSummary: summary, mode: launchMode, demo: judgeDemo },
  );
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
      for (const selector of [".panel-head", ".panel-body", ".panel-status"]) {
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
    page.locator(".sys-text", { hasText: expectedSystem }).waitFor(),
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
  assert.deepEqual(await page.locator(".panel-grok .meter-label").allTextContents(), [
    "7D",
    "MONTH",
  ]);
  assert.equal(await page.locator(".panel-grok .panel-plan").textContent(), "SUPERGROK HEAVY");

  await page.locator(".tm-settings").click();
  await page.locator(".settings-dialog").waitFor();
  assert.equal(await page.locator('.provider-option input[type="checkbox"]:checked').count(), 4);
  await page.screenshot({ path: join(artifactDir, "provider-settings.png"), fullPage: true });
  await page.locator(".provider-option.provider-claude").click();
  await page.locator(".settings-save").click();
  await page.locator(".settings-dialog").waitFor({ state: "detached" });
  assert.equal(await page.locator(".panel").count(), 3);
  assert.equal(await page.locator(".panel-claude").count(), 0);
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
    { codex: true, claude: false, deepseek: true, grok: true },
    { codex: true, claude: false, deepseek: false, grok: true },
    { codex: false, claude: false, deepseek: false, grok: true },
    { codex: false, claude: false, deepseek: false, grok: false },
    { codex: true, claude: false, deepseek: false, grok: false },
    { codex: true, claude: true, deepseek: true, grok: true },
  ]);
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
        { expectHiddenCursor: true },
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

async function checkUsageVariants(browser) {
  const variants = [
    {
      name: "claude-weekly-only",
      summary: {
        ...baseSummary,
        services: {
          ...baseSummary.services,
          claude: {
            status: "NOMINAL",
            fromCache: false,
            dataMayBeStale: false,
            sevenDayPercent: 28,
            sevenDayResetLocal: "07-29 09:30",
          },
        },
      },
      claudeLabels: ["7D"],
    },
    {
      name: "claude-extra-only",
      summary: {
        ...baseSummary,
        services: {
          ...baseSummary.services,
          claude: {
            status: "NOMINAL",
            fromCache: false,
            dataMayBeStale: false,
            extraUsagePercent: 12.5,
          },
        },
      },
      claudeLabels: ["EXTRA"],
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

    assert.deepEqual(await page.locator(".panel-codex .meter-label").allTextContents(), ["7D"]);
    assert.equal(await page.locator(".panel-codex .reset-credits").count(), 1);
    await page.locator(".panel-codex .reset-credits", { hasText: "3 AVAILABLE" }).waitFor();
    await page.locator(".panel-codex .reset-credits", { hasText: "FIRST EXP" }).waitFor();
    assert.deepEqual(
      await page.locator(".panel-claude .meter-label").allTextContents(),
      variant.claudeLabels,
    );
    assert.deepEqual(await page.locator(".panel-grok .meter-label").allTextContents(), [
      "7D",
      "MONTH",
    ]);
    assert.deepEqual(await inspectLayout(page, viewport), []);
    assert.deepEqual(pageErrors, [], `${variant.name} page errors`);
    await page.screenshot({ path: join(artifactDir, `${variant.name}.png`), fullPage: true });
    await context.close();
  }
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

async function launchTestBrowser() {
  try {
    return await chromium.launch({ headless: true });
  } catch (error) {
    if (process.platform !== "win32") throw error;
    return chromium.launch({ channel: "msedge", headless: true });
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
  await checkFullscreenThreeColumns(browser);
  process.stdout.write(`PASS fullscreen three-column layout across Surface/Full HD sizes\n`);
  await checkUsageVariants(browser);
  process.stdout.write(`PASS dynamic Codex and Claude usage-window variants\n`);
  await checkJudgeDemo(browser);
  process.stdout.write(`PASS isolated judge demo across Surface/compact/Snap layouts\n`);
  await checkKeyboardAndScreensaver(browser);
  process.stdout.write(`PASS keyboard and screensaver input\nScreenshots: ${artifactDir}\n`);
} finally {
  await browser?.close();
  if (server.exitCode === null) server.kill("SIGTERM");
}
