import { useCallback, useEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronDown, Maximize2, Minimize2, RefreshCw } from "lucide-react";
import codexIcon from "./assets/codex-color-no-bg.svg";
import {
  nextProvider,
  nextViewMode,
  readProvider,
  readViewMode,
  saveProvider,
  saveViewMode,
} from "./provider";
import type { ProviderId, ViewMode } from "./provider";
import type { ProviderLimit, ProviderLimits, ProviderStats } from "./types";

const NAMES: Record<string, string> = {
  "claude-sonnet-4-6": "Sonnet 4.6",
  "claude-opus-4-8": "Opus 4.8",
  "claude-opus-4-7": "Opus 4.7",
  "claude-opus-4-6": "Opus 4.6",
  "claude-haiku-4-5-20251001": "Haiku 4.5",
};
const COLORS: Record<string, string> = {
  "claude-sonnet-4-6": "#8B6FBF",
  "claude-opus-4-8": "#4AC9A0",
  "claude-opus-4-7": "#4A90D9",
  "claude-opus-4-6": "#C95A8B",
  "claude-haiku-4-5-20251001": "#D9844A",
};
const FALLBACKS = ["#8B6FBF", "#4A90D9", "#4AC9A0", "#D9844A", "#C95A8B"];

function modelLabel(key: string): string {
  if (NAMES[key]) return NAMES[key];
  return key
    .replace(/^claude-/, "")
    .replace(/^gpt-/, "GPT ")
    .replace(/-\d{8,}$/, "")
    .replace(/-/g, " ")
    .replace(/\b\w/g, (character) => character.toUpperCase());
}

function modelColor(key: string, index: number): string {
  return COLORS[key] ?? FALLBACKS[index % FALLBACKS.length];
}

function compact(value: number): string {
  if (value >= 1e6) return `${(value / 1e6).toFixed(1).replace(/\.0$/, "")}M`;
  if (value >= 1e3) return `${(value / 1e3).toFixed(1).replace(/\.0$/, "")}K`;
  return String(value);
}

function mmdd(value: string): string {
  const parts = value.split("-");
  return parts.length === 3 ? `${parts[1]}/${parts[2]}` : value;
}

function fmtKST(ms: number): string {
  return (
    new Date(ms).toLocaleTimeString("en-GB", {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      timeZone: "Asia/Seoul",
    }) + " KST"
  );
}

function formatReset(isoString: string): string {
  const date = new Date(isoString);
  const now = new Date();
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const diffHours = (date.getTime() - now.getTime()) / 3_600_000;
  const minutes = date.getMinutes();

  if (diffHours <= 24) {
    const time = date
      .toLocaleTimeString("en-US", {
        hour: "numeric",
        minute: minutes === 0 ? undefined : "2-digit",
        hour12: true,
      })
      .replace(/ ([AP]M)/i, (_, marker: string) => marker.toLowerCase());
    return `Resets ${time} (${timezone})`;
  }
  const value = date
    .toLocaleString("en-US", {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: minutes === 0 ? undefined : "2-digit",
      hour12: true,
    })
    .replace(/ ([AP]M)/i, (_, marker: string) => marker.toLowerCase());
  return `Resets ${value} (${timezone})`;
}

function ClaudeIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="16"
      height="16"
      fill="#D97757"
      xmlns="http://www.w3.org/2000/svg"
      style={{ flexShrink: 0 }}
    >
      <path d="m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z" />
    </svg>
  );
}

function ProviderIcon({ provider }: { provider: ProviderId }) {
  return provider === "claude" ? (
    <ClaudeIcon />
  ) : (
    <img className="provider-icon" src={codexIcon} alt="" aria-hidden="true" />
  );
}

function barColor(percent: number): string {
  if (percent >= 90) return "#C95A8B";
  if (percent >= 70) return "#D9844A";
  return "#8B6FBF";
}

function UsageBar({ window }: { window: ProviderLimit }) {
  const percent = window.utilization ?? 0;
  return (
    <div className="usage-section">
      <div className="usage-header">
        <span className="usage-title">{window.title}</span>
        <span className="usage-pct">{Math.floor(percent)}% used</span>
      </div>
      <div className="track usage-track">
        <div
          className="fill usage-fill"
          style={{
            width: `${Math.max(0.5, percent)}%`,
            background: barColor(percent),
          }}
        />
      </div>
      <div className="usage-reset">
        {window.resetsAt ? formatReset(window.resetsAt) : "No recent usage"}
      </div>
    </div>
  );
}

export default function App() {
  const [provider, setProvider] = useState<ProviderId>(readProvider);
  const [view, setView] = useState<ViewMode>(readViewMode);
  const [limits, setLimits] = useState<ProviderLimits | null>(null);
  const [limitsErr, setLimitsErr] = useState<string | null>(null);
  const [stats, setStats] = useState<ProviderStats | null>(null);
  const [loadingLimits, setLoadingLimits] = useState(false);
  const [loadingStats, setLoadingStats] = useState(false);
  const [refreshedAt, setRefreshedAt] = useState<number | null>(null);
  const [refreshedVisible, setRefreshedVisible] = useState(false);
  const providerRef = useRef(provider);
  const cardRef = useRef<HTMLDivElement>(null);
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  const isRefreshing = loadingLimits || loadingStats;
  const isSuperCompact = view === "superCompact";
  const isDetailed = view === "detailed";

  const selectProvider = useCallback((next: ProviderId) => {
    providerRef.current = next;
    saveProvider(next);
    setProvider(next);
    setLimits(null);
    setStats(null);
    setLimitsErr(null);
  }, []);

  const selectView = useCallback((next: ViewMode) => {
    saveViewMode(next);
    setView(next);
  }, []);

  const loadLimits = useCallback(async () => {
    const requestedProvider = provider;
    setLoadingLimits(true);
    try {
      const data = await invoke<ProviderLimits>("get_provider_limits", {
        provider: requestedProvider,
      });
      if (providerRef.current !== requestedProvider) return;
      setLimits(data);
      setLimitsErr(null);
      setRefreshedAt(Date.now());
      setRefreshedVisible(true);
      clearTimeout(hideTimerRef.current);
      hideTimerRef.current = setTimeout(() => setRefreshedVisible(false), 3500);
    } catch (error) {
      if (providerRef.current === requestedProvider) {
        setLimitsErr(String(error));
      }
    } finally {
      if (providerRef.current === requestedProvider) setLoadingLimits(false);
    }
  }, [provider]);

  const loadStats = useCallback(async () => {
    const requestedProvider = provider;
    setLoadingStats(true);
    try {
      const data = await invoke<ProviderStats>("get_provider_stats", {
        provider: requestedProvider,
      });
      if (providerRef.current === requestedProvider) setStats(data);
    } catch {
      // Keep the last rendered local detail during transient provider errors.
    } finally {
      if (providerRef.current === requestedProvider) setLoadingStats(false);
    }
  }, [provider]);

  useEffect(() => {
    loadLimits();
    const interval = setInterval(loadLimits, 300_000);
    return () => clearInterval(interval);
  }, [loadLimits]);

  useEffect(() => {
    loadStats();
    const interval = setInterval(loadStats, 60_000);
    return () => clearInterval(interval);
  }, [loadStats]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<ProviderId>("usage-updated", ({ payload }) => {
      if (payload === providerRef.current) loadStats();
    }).then((dispose) => {
      unlisten = dispose;
    });
    return () => unlisten?.();
  }, [loadStats]);

  useEffect(() => {
    const disposers: Array<() => void> = [];
    Promise.all([
      listen("provider-shortcut", () => {
        selectProvider(nextProvider(providerRef.current));
      }),
      listen("view-shortcut", () => {
        setView((current) => {
          const next = nextViewMode(current);
          saveViewMode(next);
          return next;
        });
      }),
    ]).then((listeners) => disposers.push(...listeners));
    return () => disposers.forEach((dispose) => dispose());
  }, [selectProvider]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (focused) {
          loadStats();
          loadLimits();
        }
      })
      .then((dispose) => {
        unlisten = dispose;
      });
    return () => unlisten?.();
  }, [loadLimits, loadStats]);

  useEffect(() => () => clearTimeout(hideTimerRef.current), []);

  useEffect(() => {
    const element = cardRef.current;
    if (!element) return;
    const height = Math.ceil(element.getBoundingClientRect().height);
    if (height > 0) invoke("set_height", { h: height }).catch(() => undefined);
  }, [limits, limitsErr, stats]);

  useEffect(() => {
    let animationFrame: number;
    const deadline = Date.now() + 650;
    let lastHeight = 0;
    const poll = () => {
      const element = cardRef.current;
      if (element) {
        const height = Math.ceil(element.getBoundingClientRect().height);
        if (height > 0 && height !== lastHeight) {
          lastHeight = height;
          invoke("set_height", { h: height }).catch(() => undefined);
        }
      }
      if (Date.now() < deadline) animationFrame = requestAnimationFrame(poll);
    };
    animationFrame = requestAnimationFrame(poll);
    return () => cancelAnimationFrame(animationFrame);
  }, [view, refreshedVisible]);

  const onCardMouseDown = (event: ReactMouseEvent) => {
    if (event.button !== 0) return;
    if (event.target instanceof Element && event.target.closest("button"))
      return;
    invoke("begin_drag").catch(() => undefined);
    getCurrentWindow()
      .startDragging()
      .catch(() => undefined);
  };

  const handleRefresh = async () => {
    if (!isRefreshing) await Promise.all([loadStats(), loadLimits()]);
  };

  const breakdownRows =
    stats?.breakdown
      .filter((row) => row.key !== "<synthetic>" && row.value > 0)
      .slice(0, 5) ?? [];
  const totalBreakdown = breakdownRows.reduce((sum, row) => sum + row.value, 0);
  const maxBreakdown = breakdownRows[0]?.value ?? 1;
  const days = stats?.daily14 ?? [];
  const barWidth = 4;
  const gap = 3;
  const svgHeight = 40;
  const maxActivity = Math.max(1, ...days.map((day) => day.value));
  const svgWidth = Math.max(
    1,
    days.length * barWidth + Math.max(0, days.length - 1) * gap,
  );

  return (
    <div className="card" ref={cardRef} onMouseDown={onCardMouseDown}>
      <div className="header">
        <button
          className="title provider-switch"
          onClick={() => selectProvider(nextProvider(provider))}
          title="Switch provider (⌃⌥P)"
        >
          <ProviderIcon provider={provider} />
          <span className={`title-text${isSuperCompact ? " hidden" : ""}`}>
            {provider === "claude" ? "Claude Code" : "Codex"}
          </span>
        </button>
        <div className="header-btns">
          <button
            className="header-btn"
            onClick={handleRefresh}
            disabled={isRefreshing}
            title="Refresh"
          >
            <RefreshCw
              size={11}
              className={isRefreshing ? "spinning" : undefined}
            />
          </button>
          <button
            className="header-btn"
            onClick={() =>
              selectView(isSuperCompact ? "compact" : "superCompact")
            }
            title={
              isSuperCompact ? "Compact view (⌃⌥V)" : "Super compact view (⌃⌥V)"
            }
          >
            {isSuperCompact ? <Maximize2 size={11} /> : <Minimize2 size={11} />}
          </button>
          <button
            className="expand-btn"
            onClick={() => selectView(isDetailed ? "compact" : "detailed")}
            title={isDetailed ? "Compact view (⌃⌥V)" : "Detailed view (⌃⌥V)"}
          >
            <ChevronDown
              size={11}
              className={`chevron ${isDetailed ? "chevron-up" : ""}`}
            />
          </button>
        </div>
      </div>

      <div
        className={`mode-panel ultra-panel${isSuperCompact ? " visible" : ""}`}
      >
        <div
          className={`mode-panel-inner${loadingLimits ? " refreshing" : ""}`}
        >
          <div className="ultra-bars">
            {(limits?.windows ?? []).map((window) => {
              const percent = window.utilization ?? 0;
              return (
                <div key={window.id} className="track ultra-track">
                  <div
                    className="fill"
                    style={{
                      width: `${Math.max(0.5, percent)}%`,
                      background: barColor(percent),
                    }}
                  />
                </div>
              );
            })}
          </div>
        </div>
      </div>

      <div
        className={`mode-panel normal-panel${isSuperCompact ? "" : " visible"}`}
      >
        <div
          className={`mode-panel-inner${loadingLimits ? " refreshing" : ""}`}
        >
          {limitsErr ? (
            <div className="empty limits-err">{limitsErr}</div>
          ) : !limits ? (
            <div className="empty">Loading…</div>
          ) : (
            limits.windows.map((window) => (
              <UsageBar key={window.id} window={window} />
            ))
          )}
          <div
            className={`refreshed-wrapper${refreshedVisible ? " visible" : ""}`}
          >
            <div className="refreshed-inner">
              <div className="refreshed-at">
                {refreshedAt === null
                  ? ""
                  : `${limits?.stale ? "Cached" : "Refreshed"} ${fmtKST(
                      refreshedAt,
                    )}`}
              </div>
            </div>
          </div>
        </div>
      </div>

      <div className={`details-wrapper${isDetailed ? " open" : ""}`}>
        <div className="details-inner">
          <div className="divider" />
          {stats ? (
            <>
              <div className={`stats${loadingStats ? " refreshing" : ""}`}>
                {stats.metrics.slice(0, 3).map((metric) => (
                  <div className="stat" key={metric.label}>
                    <div className="stat-value">{compact(metric.value)}</div>
                    <div className="stat-label">{metric.label}</div>
                  </div>
                ))}
              </div>

              {days.length > 0 && (
                <>
                  <div className="section-label" style={{ marginTop: 14 }}>
                    {stats.activityLabel}
                  </div>
                  <svg
                    className="spark"
                    width="100%"
                    height={svgHeight}
                    viewBox={`0 0 ${svgWidth} ${svgHeight}`}
                    preserveAspectRatio="none"
                  >
                    <defs>
                      <linearGradient id="sg" x1="0" y1="0" x2="0" y2="1">
                        <stop offset="0%" stopColor="#8B6FBF" />
                        <stop offset="100%" stopColor="#4A90D9" />
                      </linearGradient>
                    </defs>
                    {days.map((day, index) => {
                      const height = Math.max(
                        2,
                        (day.value / maxActivity) * svgHeight,
                      );
                      return (
                        <rect
                          key={day.date}
                          x={index * (barWidth + gap)}
                          y={svgHeight - height}
                          width={barWidth}
                          height={height}
                          rx={2}
                          ry={2}
                          fill="url(#sg)"
                        />
                      );
                    })}
                  </svg>
                  <div className="spark-dates">
                    <span>{mmdd(days[0].date)}</span>
                    <span>{mmdd(days[days.length - 1].date)}</span>
                  </div>
                </>
              )}

              {breakdownRows.length > 0 && (
                <>
                  <div className="divider" />
                  <div className="section-label">{stats.breakdownLabel}</div>
                  {breakdownRows.map((row, index) => {
                    const percent = (row.value / maxBreakdown) * 100;
                    const share = (
                      (row.value / (totalBreakdown || 1)) *
                      100
                    ).toFixed(0);
                    return (
                      <div className="token-row" key={row.key}>
                        <div className="token-meta">
                          <span className="token-name">
                            {modelLabel(row.key)}
                          </span>
                          <span className="token-val">
                            {compact(row.value)} · {share}%
                          </span>
                        </div>
                        <div className="track">
                          <div
                            className="fill"
                            style={{
                              width: `${Math.max(2, percent)}%`,
                              background: modelColor(row.key, index),
                            }}
                          />
                        </div>
                      </div>
                    );
                  })}
                </>
              )}

              <div className="divider" />
              <div className="footer">
                {stats.footer.map((item, index) => (
                  <span key={item}>
                    {index > 0 && <span className="sep">•&nbsp; </span>}
                    {item}
                  </span>
                ))}
                {stats.since && (
                  <span>
                    <span className="sep">•&nbsp; </span>
                    Since {stats.since}
                  </span>
                )}
                {stats.dataScope && (
                  <span>
                    <span className="sep">•&nbsp; </span>
                    {stats.dataScope}
                  </span>
                )}
              </div>
            </>
          ) : (
            <div className="empty">Loading details…</div>
          )}
        </div>
      </div>
    </div>
  );
}
