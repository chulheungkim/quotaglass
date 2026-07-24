import type { ProviderId } from "./provider";

export interface ProviderLimit {
  id: string;
  title: string;
  utilization: number | null;
  resetsAt: string | null;
  windowMinutes: number | null;
}

export interface ProviderLimits {
  provider: ProviderId;
  windows: ProviderLimit[];
  stale: boolean;
  cachedAt: number | null;
  plan: string | null;
  creditBalance: string | null;
}

export interface SummaryMetric {
  label: string;
  value: number;
}

export interface ActivityPoint {
  date: string;
  value: number;
}

export interface BreakdownRow {
  key: string;
  value: number;
}

export interface ProviderStats {
  provider: ProviderId;
  metrics: SummaryMetric[];
  activityLabel: string;
  daily14: ActivityPoint[];
  breakdownLabel: string;
  breakdown: BreakdownRow[];
  footer: string[];
  since: string | null;
  dataScope: string | null;
  lastUpdated: string;
}
