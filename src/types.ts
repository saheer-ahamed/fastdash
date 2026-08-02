// Mirrors the serde output of the Rust `engine` types (camelCase).

export type Health =
  | { state: "ok" }
  | { state: "needsAuth"; message: string }
  | { state: "rateLimited"; retryAfterSecs: number | null }
  | { state: "misconfigured"; message: string }
  | { state: "error"; message: string };

export interface Stat {
  label: string;
  value: string;
  sub: string | null;
}

export interface Bar {
  label: string;
  value: number;
  display: string | null;
}

export interface Column {
  key: string;
  label: string;
  numeric: boolean;
  /** One sentence saying exactly what the column counts, shown on hover. */
  hint: string | null;
}

export interface Cell {
  text: string;
  href: string | null;
  /** Sort key for cells whose text sorts wrong ("1.2M", "+10 / -2", "Jul 24, 14:30"). */
  sort: number | null;
}

export interface ListItem {
  title: string;
  subtitle: string | null;
  meta: string | null;
  href: string | null;
}

export interface HeatDay {
  date: string;
  /** Shade bucket, 0 (empty) ..= 4 (most active). */
  level: number;
  tooltip: string;
}

export interface HeatmapYear {
  label: string;
  summary: string;
  /** Ascending and contiguous, zero days included. */
  days: HeatDay[];
}

export type Panel =
  | { kind: "statCards"; title: string | null; stats: Stat[] }
  | { kind: "heading"; title: string; badge: string | null }
  | {
      kind: "meter";
      label: string;
      used: number;
      limit: number | null;
      unit: string;
      sub: string | null;
      caption: string | null;
    }
  | { kind: "table"; title: string | null; columns: Column[]; rows: Cell[][] }
  | { kind: "barList"; title: string | null; bars: Bar[] }
  | { kind: "list"; title: string | null; items: ListItem[] }
  | { kind: "note"; title: string | null; message: string }
  | { kind: "heatmap"; title: string | null; years: HeatmapYear[] };

// Inclusive span of calendar days (`YYYY-MM-DD`), mirroring `engine::range::DateRange`.
export interface DateRange {
  start: string;
  end: string;
}

export interface Snapshot {
  status: Health;
  panels: Panel[];
  fetchedAt: string;
  nextRefreshSecs: number | null;
}

export interface ConnectorMeta {
  id: string;
  name: string;
  icon: string;
  defaultRefreshSecs: number;
}

// Payload of the `connector:update` Tauri event emitted by the scheduler. The
// range says which days the snapshot covers, so a background refresh is filed
// under the right cache slot instead of overwriting another range on screen.
export interface ConnectorUpdate {
  id: string;
  range: DateRange;
  snapshot: Snapshot;
}

// Mirrors the serde output of `engine::config::AppConfig` (camelCase).
export interface GithubAccount {
  label: string;
  orgs: string[];
}

export interface ClaudeConfig {
  /** Console org the stored Admin key belongs to; empty when not connected. */
  consoleOrg: string;
}

export interface AppConfig {
  timezone: string;
  locale: string;
  github: { accounts: GithubAccount[] };
  claude: ClaudeConfig;
  filterBots: boolean;
}
