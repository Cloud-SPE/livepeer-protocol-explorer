export interface RegionEntry {
  id: string;
  name: string;
  type: string;
}

export interface RegionsResponse {
  regions: RegionEntry[];
}

export interface PipelineEntry {
  id: string;
  models: string[];
  regions: string[];
}

export interface PipelinesResponse {
  pipelines: PipelineEntry[];
}

export interface LeaderboardScore {
  success_rate: number;
  round_trip_score: number;
  score: number;
}

/** Aggregated stats keyed by orchestrator address → region → score. */
export type LeaderboardResponse = Record<string, Record<string, LeaderboardScore>>;

export interface PerfDataPoint {
  region: string;
  orchestrator: string;
  success_rate: number;
  round_trip_time: number;
  errors: string[];
  timestamp: number; // unix seconds
  seg_duration: number;
  segments_sent: number;
  segments_received: number;
  upload_time: number;
  download_time: number;
  transcode_time: number;
}

/** Raw stats keyed by region → recent samples for one orchestrator. */
export type PerfStatsResponse = Record<string, PerfDataPoint[]>;
