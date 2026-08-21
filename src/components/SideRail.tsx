import type { CSSProperties } from "react";
import { IP_THEME } from "../ipTheme";
import { PROVIDER_ORDER } from "../planLayout";
import type { EnabledProviders } from "../types";

// Decorative left rail. Ring skin keeps the HUD ticks; IP skin is a field-color index.
export function SideRail({ enabledProviders }: { enabledProviders: EnabledProviders }) {
  return (
    <aside className="rail" aria-hidden>
      <span className="rail-rec">● LIVE</span>
      <span className="rail-text">AI · USAGE · MONITOR</span>
      <span className="rail-hex" />
      <span className="rail-ticks">
        {Array.from({ length: 9 }).map((_, i) => (
          <i key={i} />
        ))}
      </span>
      <span className="rail-code">MGI-09</span>
      <div className="rail-fields">
        {PROVIDER_ORDER.map((kind) => (
          <i
            key={kind}
            className="rail-field"
            data-kind={kind}
            data-on={enabledProviders[kind] ? "true" : "false"}
            style={
              {
                background: IP_THEME[kind].field,
                "--swatch": IP_THEME[kind].field,
              } as CSSProperties
            }
          />
        ))}
      </div>
    </aside>
  );
}
