import { Check, KeyRound, Save, X } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import type { EnabledProviders, WindowMode } from "../types";

const PROVIDERS: Array<{ key: keyof EnabledProviders; label: string; code: string }> = [
  { key: "codex", label: "CODEX", code: "SYS-01" },
  { key: "claude", label: "CLAUDE", code: "SYS-02" },
  { key: "deepseek", label: "DEEPSEEK", code: "SYS-03" },
  { key: "grok", label: "GROK", code: "SYS-04" },
];

const WINDOW_MODES: Array<{ value: WindowMode; label: string; code: string }> = [
  { value: "normal", label: "WINDOWED", code: "WIN" },
  { value: "fullscreen", label: "FULLSCREEN", code: "FULL" },
];

export function ProviderSettings({
  value,
  windowMode,
  judgeDemo,
  onClose,
  onSave,
}: {
  value: EnabledProviders;
  /** Current effective mode; screensaver never renders this dialog. */
  windowMode: WindowMode;
  judgeDemo: boolean;
  onClose: () => void;
  onSave: (
    value?: EnabledProviders,
    windowMode?: WindowMode,
    deepseekApiKey?: string,
  ) => Promise<void>;
}) {
  const [draft, setDraft] = useState(value);
  const [draftWindow, setDraftWindow] = useState<WindowMode>(windowMode);
  const [windowModeTouched, setWindowModeTouched] = useState(false);
  const [deepseekApiKey, setDeepseekApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const savingRef = useRef(false);
  const closeRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useRef<HTMLElement>(null);

  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  useEffect(() => {
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      if (!savingRef.current) onClose();
    };
    document.addEventListener("keydown", handleEscape, true);
    return () => document.removeEventListener("keydown", handleEscape, true);
  }, [onClose]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    setError(null);
    try {
      const providersChanged = PROVIDERS.some(({ key }) => draft[key] !== value[key]);
      const keyToSave = deepseekApiKey.trim();
      if (providersChanged || windowModeTouched || keyToSave) {
        await onSave(
          providersChanged ? draft : undefined,
          windowModeTouched ? draftWindow : undefined,
          keyToSave || undefined,
        );
      }
      setDeepseekApiKey("");
      onClose();
    } catch (err) {
      console.error("settings save failed", err);
      setError("SETTINGS SAVE FAILED");
      savingRef.current = false;
      setSaving(false);
    }
  };

  const handleDialogKeyDown = (event: React.KeyboardEvent) => {
    if (event.key !== "Tab") return;

    const focusable = Array.from(
      dialogRef.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), [href], [tabindex]:not([tabindex="-1"])',
      ) ?? [],
    ).filter((element) => element.getClientRects().length > 0);
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      className="settings-backdrop"
      onKeyDown={handleDialogKeyDown}
      onPointerDown={(event) => {
        if (event.target === event.currentTarget && !saving) onClose();
      }}
    >
      <section
        ref={dialogRef}
        className="settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="provider-settings-title"
      >
        <header className="settings-head">
          <div>
            <span className="settings-kicker">HOME DISPLAY</span>
            <h2 id="provider-settings-title">DISPLAY SETTINGS</h2>
          </div>
          <button
            ref={closeRef}
            type="button"
            className="icon-button settings-close"
            onClick={onClose}
            disabled={saving}
            aria-label="Close settings"
            title="Close settings"
          >
            <X size={19} aria-hidden />
          </button>
        </header>

        <form onSubmit={submit}>
          <fieldset className="window-mode-options" disabled={saving}>
            <legend className="settings-section-label">WINDOW MODE</legend>
            <div className="window-mode-grid" role="radiogroup" aria-label="Window mode">
              {WINDOW_MODES.map((mode) => (
                <label
                  className={`window-mode-option${draftWindow === mode.value ? " is-selected" : ""}`}
                  key={mode.value}
                >
                  <input
                    type="radio"
                    name="windowMode"
                    value={mode.value}
                    checked={draftWindow === mode.value}
                    onClick={() => setWindowModeTouched(true)}
                    onChange={() => {
                      setDraftWindow(mode.value);
                      setWindowModeTouched(true);
                    }}
                  />
                  <span className="window-mode-check" aria-hidden>
                    <Check size={14} strokeWidth={3} />
                  </span>
                  <span className="window-mode-name">{mode.label}</span>
                  <span className="window-mode-code">{mode.code}</span>
                </label>
              ))}
            </div>
          </fieldset>

          <fieldset className="provider-options" disabled={saving}>
            <legend className="settings-section-label">ACTIVE PROVIDERS</legend>
            {PROVIDERS.map((provider) => (
              <label className={`provider-option provider-${provider.key}`} key={provider.key}>
                <input
                  type="checkbox"
                  checked={draft[provider.key]}
                  onChange={(event) =>
                    setDraft((current) => ({
                      ...current,
                      [provider.key]: event.target.checked,
                    }))
                  }
                />
                <span className="provider-check" aria-hidden>
                  <Check size={16} strokeWidth={3} />
                </span>
                <span className="provider-name">{provider.label}</span>
                <span className="provider-code">{provider.code}</span>
              </label>
            ))}
          </fieldset>

          <fieldset className="credential-options" disabled={saving || judgeDemo}>
            <legend className="settings-section-label">DEEPSEEK API KEY</legend>
            <label className="credential-field" htmlFor="deepseek-api-key">
              <span className="credential-input-shell">
                <KeyRound size={17} aria-hidden />
                <input
                  id="deepseek-api-key"
                  name="deepseek-api-key-new"
                  type="password"
                  value={deepseekApiKey}
                  onChange={(event) => setDeepseekApiKey(event.target.value)}
                  autoComplete="new-password"
                  spellCheck={false}
                  maxLength={2_048}
                  placeholder={judgeDemo ? "DISABLED IN SYNTHETIC DEMO" : "PASTE KEY TO REPLACE"}
                  aria-describedby="deepseek-key-help"
                />
              </span>
              <span id="deepseek-key-help" className="credential-help">
                {judgeDemo
                  ? "CREDENTIALS ARE NEVER SAVED IN DEMO MODE"
                  : "LEAVE BLANK TO KEEP CURRENT KEY · SAVED IN PER-USER CONFIG"}
              </span>
            </label>
          </fieldset>

          <footer className="settings-actions">
            {error && (
              <span className="settings-error" role="alert">
                {error}
              </span>
            )}
            <button type="button" className="settings-cancel" onClick={onClose} disabled={saving}>
              CANCEL
            </button>
            <button type="submit" className="settings-save" disabled={saving}>
              <Save size={16} aria-hidden />
              {saving ? "SAVING" : "SAVE"}
            </button>
          </footer>
        </form>
      </section>
    </div>
  );
}
