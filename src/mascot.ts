import type { ArtSkin } from "./ipTheme";
import type { ProviderKind } from "./planLayout";

export type MascotMood = "idle" | "focus" | "tense" | "over" | "error";

const AUTH_STATUS =
  /AUTH|LOGIN|REFRESH|KEY MISSING|NOT CONFIG|CREDENTIAL|SESSION EXPIRED|ACCESS UNAVAILABLE|TEAM USAGE UNAVAILABLE/i;
const OVER_STATUS = /INSUFFICIENT|DEPLETED|EXHAUSTED/i;

const artIp = import.meta.glob("./assets/mascots-ip/*.png", {
  eager: true,
  import: "default",
}) as Record<string, string>;

export function mascotMood(
  status: string,
  percents: Array<number | undefined>,
): MascotMood {
  if (AUTH_STATUS.test(status)) {
    return "error";
  }
  if (OVER_STATUS.test(status)) {
    return "over";
  }
  const peak = peakPercent(percents);
  if (peak == null) {
    return "idle";
  }
  if (peak >= 88) {
    return "over";
  }
  if (peak >= 70) {
    return "tense";
  }
  if (peak >= 35) {
    return "focus";
  }
  return "idle";
}

export function peakPercent(percents: Array<number | undefined>): number | null {
  const values = percents.filter((value): value is number =>
    typeof value === "number" && Number.isFinite(value),
  );
  if (values.length === 0) {
    return null;
  }
  return Math.max(...values);
}

export function mascotSrc(
  kind: ProviderKind,
  mood: MascotMood,
  skin: ArtSkin = "ip",
): string {
  if (skin !== "ip") {
    return "";
  }
  return artIp[`./assets/mascots-ip/${kind}-${mood}.png`] ?? "";
}

export function moodColorVar(mood: MascotMood): "--accent" | "--warn" | "--danger" {
  if (mood === "over" || mood === "error") {
    return "--danger";
  }
  if (mood === "tense") {
    return "--warn";
  }
  return "--accent";
}
