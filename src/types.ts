export interface DayStat {
  date: string;
  messages: number;
  sessions: number;
  tools: number;
}

export interface Today {
  date: string;
  messages: number;
  sessions: number;
  toolCalls: number;
}

export interface AllTime {
  sessions: number;
  messages: number;
}

export interface UsageStats {
  today: Today;
  daily14: DayStat[];
  modelTokens: Record<string, number>;
  allTime: AllTime;
  since: string | null;
  lastUpdated: string;
}
