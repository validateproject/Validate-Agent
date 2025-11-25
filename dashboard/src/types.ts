export interface ValidatorMetrics {
  slot_lag: number;
  vote_success_rate: number;
  cpu_usage: number;
  ram_usage_gb: number;
  disk_usage_pct: number;
  rpc_qps: number;
  rpc_error_rate: number;
  last_updated: number;
}

export interface ValidatorSummary {
  id: string;
  host: string;
  prometheus_url: string;
  metrics: ValidatorMetrics | null;
  status: string;
  risk_score: number | null;
}

export interface ValidatorsResponse {
  validators: ValidatorSummary[];
}

export interface ActionsResponse {
  pending: number;
}

