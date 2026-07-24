export const PROVIDERS = ["claude", "codex"] as const;
export type ProviderId = (typeof PROVIDERS)[number];

export const VIEW_MODES = ["superCompact", "compact", "detailed"] as const;
export type ViewMode = (typeof VIEW_MODES)[number];

export function nextProvider(provider: ProviderId): ProviderId {
  return provider === "claude" ? "codex" : "claude";
}

export function nextViewMode(view: ViewMode): ViewMode {
  const index = VIEW_MODES.indexOf(view);
  return VIEW_MODES[(index + 1) % VIEW_MODES.length];
}

export function readProvider(): ProviderId {
  return localStorage.getItem("widget-provider") === "codex"
    ? "codex"
    : "claude";
}

export function readViewMode(): ViewMode {
  const saved = localStorage.getItem("widget-view");
  if (saved === "superCompact" || saved === "compact" || saved === "detailed") {
    return saved;
  }
  return localStorage.getItem("widget-ultra") === "1"
    ? "superCompact"
    : "compact";
}

export function saveProvider(provider: ProviderId): void {
  localStorage.setItem("widget-provider", provider);
}

export function saveViewMode(view: ViewMode): void {
  localStorage.setItem("widget-view", view);
}
