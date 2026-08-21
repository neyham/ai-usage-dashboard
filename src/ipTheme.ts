import type { CSSProperties } from "react";
import type { ProviderKind } from "./planLayout";

export type ArtSkin = "ring" | "ip";

export interface IpTheme {
  field: string;
  ipA: string;
  ipB: string;
  ringOk: string;
  onField: string;
  emerge: "left" | "right";
}

export const IP_THEME: Record<ProviderKind, IpTheme> = {
  codex: {
    field: "#FF3B4A",
    ipA: "#08332F",
    ipB: "#5CFFD0",
    ringOk: "#5CFFD0",
    onField: "#121212",
    emerge: "left",
  },
  claude: {
    field: "#00B8C8",
    ipA: "#6B1604",
    ipB: "#FFF0D4",
    ringOk: "#FFF0D4",
    onField: "#121212",
    emerge: "right",
  },
  deepseek: {
    field: "#FFD000",
    ipA: "#0E2F70",
    ipB: "#F2FBFF",
    ringOk: "#F2FBFF",
    onField: "#121212",
    emerge: "left",
  },
  grok: {
    field: "#5B21FF",
    ipA: "#D4FF00",
    ipB: "#161616",
    ringOk: "#D4FF00",
    onField: "#FFF8F0",
    emerge: "left",
  },
  cursor: {
    field: "#FF62D0",
    ipA: "#1C1730",
    ipB: "#F7F1FF",
    ringOk: "#F7F1FF",
    onField: "#121212",
    emerge: "left",
  },
  antigravity: {
    field: "#009B52",
    ipA: "#E7FFF6",
    ipB: "#1A2C86",
    ringOk: "#E7FFF6",
    onField: "#121212",
    emerge: "right",
  },
};

export function normalizeArtSkin(value: string | undefined | null): ArtSkin {
  return value?.trim().toLowerCase() === "ring" ? "ring" : "ip";
}

export function ipVars(kind: ProviderKind): CSSProperties {
  const theme = IP_THEME[kind];
  return {
    "--panel-field": theme.field,
    "--ip-a": theme.ipA,
    "--ip-b": theme.ipB,
    "--ring-ok": theme.ringOk,
    "--on-field": theme.onField,
  } as CSSProperties;
}
