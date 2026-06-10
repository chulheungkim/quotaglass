import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { ChevronDown, Minimize2, Maximize2 } from "lucide-react";
import type { UsageStats, RateLimits, LimitWindow } from "./types";

// ── model display helpers ─────────────────────────────────────────────────────

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
    .replace(/-\d{8,}$/, "")
    .replace(/-/g, " ")
    .replace(/\b\w/g, (c) => c.toUpperCase());
}

function modelColor(key: string, i: number): string {
  return COLORS[key] ?? FALLBACKS[i % FALLBACKS.length];
}

// ── formatters ────────────────────────────────────────────────────────────────

function compact(n: number): string {
  if (n >= 1e6) return (n / 1e6).toFixed(1).replace(/\.0$/, "") + "M";
  if (n >= 1e3) return (n / 1e3).toFixed(1).replace(/\.0$/, "") + "K";
  return String(n);
}

function fmtInt(n: number): string {
  return n.toLocaleString("en-US");
}

function mmdd(s: string): string {
  const p = s.split("-");
  return p.length === 3 ? `${p[1]}/${p[2]}` : s;
}

// Matches Claude Code's own isQ() / WGA() formatter
function formatReset(isoStr: string): string {
  const d = new Date(isoStr);
  const now = new Date();
  const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const diffHours = (d.getTime() - now.getTime()) / 3_600_000;
  const mins = d.getMinutes();

  if (diffHours <= 24) {
    const time = d
      .toLocaleTimeString("en-US", {
        hour: "numeric",
        minute: mins === 0 ? undefined : "2-digit",
        hour12: true,
      })
      .replace(/ ([AP]M)/i, (_, m: string) => m.toLowerCase());
    return `Resets ${time} (${tz})`;
  }
  const opts: Intl.DateTimeFormatOptions = {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: mins === 0 ? undefined : "2-digit",
    hour12: true,
  };
  const str = d
    .toLocaleString("en-US", opts)
    .replace(/ ([AP]M)/i, (_, m: string) => m.toLowerCase());
  return `Resets ${str} (${tz})`;
}

// ── Claude icon ───────────────────────────────────────────────────────────────

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

// ── UsageBar ──────────────────────────────────────────────────────────────────

function UsageBar({
  title,
  window: w,
}: {
  title: string;
  window: LimitWindow | null;
}) {
  if (!w) return null;
  const pct = w.utilization ?? 0;
  const resetStr = w.resetsAt ? formatReset(w.resetsAt) : null;

  let fillColor = "#8B6FBF";
  if (pct >= 90) fillColor = "#C95A8B";
  else if (pct >= 70) fillColor = "#D9844A";

  return (
    <div className="usage-section">
      <div className="usage-header">
        <span className="usage-title">{title}</span>
        <span className="usage-pct">{Math.floor(pct)}% used</span>
      </div>
      <div className="track usage-track">
        <div
          className="fill usage-fill"
          style={{ width: `${Math.max(0.5, pct)}%`, background: fillColor }}
        />
      </div>
      {resetStr && <div className="usage-reset">{resetStr}</div>}
    </div>
  );
}

// ── App ───────────────────────────────────────────────────────────────────────

export default function App() {
  const [limits, setLimits] = useState<RateLimits | null>(null);
  const [limitsErr, setLimitsErr] = useState<string | null>(null);
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [ultra, setUltra] = useState(
    () => localStorage.getItem("widget-ultra") === "1",
  );
  const cardRef = useRef<HTMLDivElement>(null);

  const toggleUltra = () =>
    setUltra((u) => {
      const next = !u;
      localStorage.setItem("widget-ultra", next ? "1" : "0");
      return next;
    });

  // Rate limits — network call every 5 min
  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const data = await invoke<RateLimits>("get_rate_limits");
        if (active) {
          setLimits(data);
          setLimitsErr(null);
        }
      } catch (e) {
        if (active) setLimitsErr(String(e));
      }
    };
    load();
    const id = setInterval(load, 300_000);
    return () => {
      active = false;
      clearInterval(id);
    };
  }, []);

  // Historical stats — local reads every 60 s (always, so expand is instant)
  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const data = await invoke<UsageStats>("get_usage_stats");
        if (active) setStats(data);
      } catch {
        /* ignore */
      }
    };
    load();
    const id = setInterval(load, 60_000);
    return () => {
      active = false;
      clearInterval(id);
    };
  }, []);

  // Immediate resize + re-anchor when data or mode changes
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    const h = Math.ceil(el.getBoundingClientRect().height);
    if (h > 0)
      getCurrentWindow()
        .setSize(new LogicalSize(300, h))
        .then(() => invoke("reanchor"))
        .catch(() => undefined);
  }, [limits, limitsErr, stats, ultra]);

  // RAF-tracked resize during expand/collapse animation, re-anchor at end
  useEffect(() => {
    let raf: number;
    const deadline = Date.now() + 420;
    let anchored = false;
    const poll = () => {
      const el = cardRef.current;
      if (el) {
        const h = Math.ceil(el.getBoundingClientRect().height);
        if (h > 0)
          getCurrentWindow()
            .setSize(new LogicalSize(300, h))
            .catch(() => undefined);
      }
      if (Date.now() < deadline) {
        raf = requestAnimationFrame(poll);
      } else if (!anchored) {
        anchored = true;
        invoke("reanchor").catch(() => undefined);
      }
    };
    raf = requestAnimationFrame(poll);
    return () => cancelAnimationFrame(raf);
  }, [expanded]);

  const tokenRows = stats
    ? Object.entries(stats.modelTokens)
        .filter(([k, v]) => k !== "<synthetic>" && v > 0)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 5)
    : [];
  const totalTok = tokenRows.reduce((s, [, v]) => s + v, 0);
  const maxTok = tokenRows.length > 0 ? tokenRows[0][1] : 1;

  const days = stats?.daily14 ?? [];
  const barW = 4,
    gap = 3,
    svgH = 40;
  const maxMsg = Math.max(1, ...days.map((d) => d.messages));
  const svgW = Math.max(
    1,
    days.length * barW + Math.max(0, days.length - 1) * gap,
  );

  return (
    <div className="card" data-tauri-drag-region ref={cardRef}>
      {/* Header */}
      <div className="header">
        <div className="title">
          <ClaudeIcon />
          <span className={`title-text${ultra ? " hidden" : ""}`}>
            Claude Code
          </span>
        </div>
        <div className="header-btns">
          {/* Ultra-compact toggle: 2 lines = compress, 3 lines = expand */}
          <button
            className="header-btn"
            onClick={toggleUltra}
            title={ultra ? "Show details" : "Ultra compact"}
          >
            {ultra ? <Maximize2 size={11} /> : <Minimize2 size={11} />}
          </button>
          <button
            className="expand-btn"
            onClick={() => setExpanded((e) => !e)}
            title={expanded ? "Collapse" : "Expand"}
          >
            <ChevronDown
              size={11}
              className={`chevron ${expanded ? "chevron-up" : ""}`}
            />
          </button>
        </div>
      </div>

      {/* Ultra-compact: 3 visual-only bars, no labels */}
      <div className={`mode-panel ultra-panel${ultra ? " visible" : ""}`}>
        <div className="ultra-bars">
          {[limits?.fiveHour, limits?.sevenDay, limits?.sevenDaySonnet].map(
            (w, i) => {
              const pct = w?.utilization ?? 0;
              let color = "#8B6FBF";
              if (pct >= 90) color = "#C95A8B";
              else if (pct >= 70) color = "#D9844A";
              return (
                <div key={i} className="track ultra-track">
                  <div
                    className="fill"
                    style={{
                      width: `${Math.max(0.5, pct)}%`,
                      background: color,
                    }}
                  />
                </div>
              );
            },
          )}
        </div>
      </div>

      {/* Normal compact: rate-limit bars with labels */}
      <div className={`mode-panel normal-panel${ultra ? "" : " visible"}`}>
        {limitsErr ? (
          <div className="empty limits-err">{limitsErr}</div>
        ) : !limits ? (
          <div className="empty">Loading…</div>
        ) : (
          <>
            <UsageBar title="Current session" window={limits.fiveHour} />
            <UsageBar
              title="Current week (all models)"
              window={limits.sevenDay}
            />
            <UsageBar
              title="Current week (Sonnet only)"
              window={limits.sevenDaySonnet}
            />
          </>
        )}
      </div>

      {/* Expandable details — always in DOM so CSS can animate height */}
      <div className={`details-wrapper${expanded ? " open" : ""}`}>
        <div className="details-inner">
          <div className="divider" />

          {stats ? (
            <>
              {/* Today quick stats */}
              <div className="stats">
                <div className="stat">
                  <div className="stat-value">
                    {compact(stats.today.messages)}
                  </div>
                  <div className="stat-label">Messages</div>
                </div>
                <div className="stat">
                  <div className="stat-value">
                    {fmtInt(stats.today.sessions)}
                  </div>
                  <div className="stat-label">Sessions</div>
                </div>
                <div className="stat">
                  <div className="stat-value">
                    {compact(stats.today.toolCalls)}
                  </div>
                  <div className="stat-label">Tools</div>
                </div>
              </div>

              {/* 14-day sparkline */}
              {days.length > 0 && (
                <>
                  <div className="section-label" style={{ marginTop: 14 }}>
                    14-Day Activity
                  </div>
                  <svg
                    className="spark"
                    width="100%"
                    height={svgH}
                    viewBox={`0 0 ${svgW} ${svgH}`}
                    preserveAspectRatio="none"
                  >
                    <defs>
                      <linearGradient id="sg" x1="0" y1="0" x2="0" y2="1">
                        <stop offset="0%" stopColor="#8B6FBF" />
                        <stop offset="100%" stopColor="#4A90D9" />
                      </linearGradient>
                    </defs>
                    {days.map((d, i) => {
                      const h = Math.max(2, (d.messages / maxMsg) * svgH);
                      return (
                        <rect
                          key={d.date}
                          x={i * (barW + gap)}
                          y={svgH - h}
                          width={barW}
                          height={h}
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

              {/* Tokens by model */}
              {tokenRows.length > 0 && (
                <>
                  <div className="divider" />
                  <div className="section-label">Tokens by Model</div>
                  {tokenRows.map(([key, val], i) => {
                    const pct = (val / maxTok) * 100;
                    const share = ((val / (totalTok || 1)) * 100).toFixed(0);
                    return (
                      <div className="token-row" key={key}>
                        <div className="token-meta">
                          <span className="token-name">{modelLabel(key)}</span>
                          <span className="token-val">
                            {compact(val)} · {share}%
                          </span>
                        </div>
                        <div className="track">
                          <div
                            className="fill"
                            style={{
                              width: `${Math.max(2, pct)}%`,
                              background: modelColor(key, i),
                            }}
                          />
                        </div>
                      </div>
                    );
                  })}
                </>
              )}

              {/* Footer */}
              <div className="divider" />
              <div className="footer">
                <span>{fmtInt(stats.allTime.sessions)} sessions</span>
                <span className="sep">•</span>
                <span>{fmtInt(stats.allTime.messages)} messages</span>
                {stats.since && (
                  <>
                    <span className="sep">•</span>
                    <span>Since {stats.since}</span>
                  </>
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
