import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import type { UsageStats } from "./types";

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

const FALLBACKS = [
  "#8B6FBF",
  "#4A90D9",
  "#4AC9A0",
  "#D9844A",
  "#C95A8B",
  "#B0B04A",
];

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

export default function App() {
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const cardRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const data = await invoke<UsageStats>("get_usage_stats");
        if (active) {
          setStats(data);
          setErr(null);
        }
      } catch (e) {
        if (active) setErr(String(e));
      }
    };
    load();
    const id = setInterval(load, 60000);
    return () => {
      active = false;
      clearInterval(id);
    };
  }, []);

  // Resize the OS window to fit the rendered card exactly.
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    const h = Math.ceil(el.getBoundingClientRect().height);
    if (h > 0) {
      getCurrentWindow()
        .setSize(new LogicalSize(300, h))
        .catch(() => undefined);
    }
  }, [stats, err]);

  const tokenRows = stats
    ? Object.entries(stats.modelTokens)
        .filter(([k, v]) => k !== "<synthetic>" && v > 0)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 5)
    : [];
  const totalTok = tokenRows.reduce((s, [, v]) => s + v, 0);
  const maxTok = tokenRows.length > 0 ? tokenRows[0][1] : 1;

  const days = stats?.daily14 ?? [];
  const barW = 4;
  const gap = 3;
  const svgH = 44;
  const maxMsg = Math.max(1, ...days.map((d) => d.messages));
  const svgW = Math.max(
    1,
    days.length * barW + Math.max(0, days.length - 1) * gap,
  );

  return (
    <div className="card" data-tauri-drag-region ref={cardRef}>
      <div className="header">
        <div className="title">
          <span className="dot">●</span>Claude Code
        </div>
        <div className="date">{stats?.lastUpdated ?? ""}</div>
      </div>

      {err ? (
        <div className="empty">{err}</div>
      ) : !stats ? (
        <div className="empty">Loading…</div>
      ) : (
        <>
          <div className="stats">
            <div className="stat">
              <div className="stat-value">{compact(stats.today.messages)}</div>
              <div className="stat-label">Messages</div>
            </div>
            <div className="stat">
              <div className="stat-value">{fmtInt(stats.today.sessions)}</div>
              <div className="stat-label">Sessions</div>
            </div>
            <div className="stat">
              <div className="stat-value">{compact(stats.today.toolCalls)}</div>
              <div className="stat-label">Tools</div>
            </div>
          </div>

          <div className="divider" />

          <div className="section-label">14-Day Activity</div>
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
            <span>{days.length > 0 ? mmdd(days[0].date) : ""}</span>
            <span>
              {days.length > 0 ? mmdd(days[days.length - 1].date) : ""}
            </span>
          </div>

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

          <div className="divider" />

          <div className="footer">
            <span>{fmtInt(stats.allTime.sessions)} sessions</span>
            <span className="sep">•</span>
            <span>{fmtInt(stats.allTime.messages)} messages</span>
            {stats.since ? (
              <>
                <span className="sep">•</span>
                <span>Since {stats.since}</span>
              </>
            ) : null}
          </div>
        </>
      )}
    </div>
  );
}
