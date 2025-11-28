export type LogLevel = "info" | "warn" | "error";

export interface LogSample {
  level: LogLevel;
  tag: string;
  message: string;
}

export interface PrometheusMetric {
  name: string;
  labels: Record<string, string>;
  value: number;
}

export interface MetricSnapshot {
  slot: number | null;
  blockHeight: number | null;
  healthOk: number | null;
  slotLag: number | null;
  voteSuccessRate: number | null;
  cpuUsage: number | null;
  ramUsageGb: number | null;
  diskUsagePct: number | null;
  ramBytes: number | null;
  virtualMemBytes: number | null;
  processCpuSeconds: number | null;
  processOpenFds: number | null;
  gcCollections: Record<string, number>;
  gcObjectsCollected: Record<string, number>;
  timestamp: number | null;
  startTime: number | null;
}

