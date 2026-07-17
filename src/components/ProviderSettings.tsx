import { Check, Save, X } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import type { EnabledProviders } from "../types";

const PROVIDERS: Array<{ key: keyof EnabledProviders; label: string; code: string }> = [
  { key: "codex", label: "CODEX", code: "SYS-01" },
  { key: "claude", label: "CLAUDE", code: "SYS-02" },
  { key: "deepseek", label: "DEEPSEEK", code: "SYS-03" },
];

export function ProviderSettings({
  value,
  onClose,
  onSave,
}: {
  value: EnabledProviders;
  onClose: () => void;
  onSave: (value: EnabledProviders) => Promise<void>;
}) {
  const [draft, setDraft] = useState(value);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      await onSave(draft);
      onClose();
    } catch (err) {
      console.error("save_enabled_providers failed", err);
      setError("SETTINGS SAVE FAILED");
      setSaving(false);
    }
  };

  return (
    <div
      className="settings-backdrop"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget && !saving) onClose();
      }}
    >
      <section
        className="settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="provider-settings-title"
      >
        <header className="settings-head">
          <div>
            <span className="settings-kicker">HOME DISPLAY</span>
            <h2 id="provider-settings-title">ACTIVE PROVIDERS</h2>
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
          <fieldset className="provider-options" disabled={saving}>
            <legend className="sr-only">Providers shown on the home screen</legend>
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

          <footer className="settings-actions">
            {error && <span className="settings-error">{error}</span>}
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
