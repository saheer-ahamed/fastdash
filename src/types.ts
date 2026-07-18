// Mirrors the serde output of the Rust `engine` types (camelCase).

export type Health =
  | { state: "ok" }
  | { state: "needsAuth"; message: string }
  | { state: "rateLimited"; retryAfterSecs: number | null }
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
}

export interface Cell {
  text: string;
  href: string | null;
}

export interface ListItem {
  title: string;
  subtitle: string | null;
  meta: string | null;
  href: string | null;
}

export type Panel =
  | { kind: "statCards"; title: string | null; stats: Stat[] }
  | {
      kind: "meter";
      label: string;
      used: number;
      limit: number | null;
      unit: string;
      caption: string | null;
    }
  | { kind: "table"; title: string | null; columns: Column[]; rows: Cell[][] }
  | { kind: "barList"; title: string | null; bars: Bar[] }
  | { kind: "list"; title: string | null; items: ListItem[] };

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

// Payload of the `connector:update` Tauri event emitted by the scheduler.
export interface ConnectorUpdate {
  id: string;
  snapshot: Snapshot;
}

// Mirrors the serde output of `engine::config::AppConfig` (camelCase).
export interface GithubAccount {
  label: string;
  orgs: string[];
}

export interface SlackWorkspace {
  label: string;
}

export interface AppConfig {
  timezone: string;
  github: { accounts: GithubAccount[] };
  slack: { workspaces: SlackWorkspace[] };
  filterBots: boolean;
}
