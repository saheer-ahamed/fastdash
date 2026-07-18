import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig } from "./types";

// Minimal settings UI: non-secret config goes to `save_config`; tokens go
// straight to the OS keychain via `set_secret` and are never read back.
// After a successful save we ask the parent to refresh that connector so the
// user sees results immediately instead of waiting for the next scheduler tick.
export default function Settings({
  onRefresh,
}: {
  onRefresh: (connectorId: string) => void;
}) {
  const [config, setConfig] = useState<AppConfig | null>(null);

  // General
  const [timezone, setTimezone] = useState("");
  const [filterBots, setFilterBots] = useState(true);

  // GitHub (single account in the minimal UI)
  const [ghLabel, setGhLabel] = useState("work");
  const [ghOrgs, setGhOrgs] = useState("");
  const [ghToken, setGhToken] = useState("");

  // Slack (single workspace in the minimal UI)
  const [slackLabel, setSlackLabel] = useState("default");
  const [slackToken, setSlackToken] = useState("");

  const [savedMsg, setSavedMsg] = useState<string | null>(null);

  useEffect(() => {
    invoke<AppConfig>("get_config")
      .then((cfg) => {
        setConfig(cfg);
        setTimezone(cfg.timezone);
        setFilterBots(cfg.filterBots);
        const acct = cfg.github.accounts[0];
        if (acct) {
          setGhLabel(acct.label);
          setGhOrgs(acct.orgs.join(", "));
        }
        const ws = cfg.slack.workspaces[0];
        if (ws) setSlackLabel(ws.label);
      })
      .catch((e) => console.error(e));
  }, []);

  const flash = (msg: string) => {
    setSavedMsg(msg);
    window.setTimeout(() => setSavedMsg(null), 2500);
  };

  // Persist the config plus whatever slices the caller overrides in one write,
  // keeping the in-memory `config` state in sync.
  async function persist(patch: Partial<AppConfig>): Promise<AppConfig> {
    const base = config ?? {
      timezone,
      github: { accounts: [] },
      slack: { workspaces: [] },
      filterBots,
    };
    const next: AppConfig = { ...base, timezone, filterBots, ...patch };
    await invoke("save_config", { config: next });
    setConfig(next);
    return next;
  }

  const parseOrgs = (raw: string): string[] =>
    raw
      .split(/[\s,]+/)
      .map((s) => s.trim())
      .filter(Boolean);

  async function saveGeneral() {
    try {
      await persist({});
      flash("General settings saved");
    } catch (e) {
      flash(`Error: ${e}`);
    }
  }

  async function saveGithub() {
    try {
      const label = ghLabel.trim() || "work";
      if (ghToken.trim()) {
        await invoke("set_secret", { connector: "github", label, value: ghToken.trim() });
        setGhToken("");
      }
      await persist({ github: { accounts: [{ label, orgs: parseOrgs(ghOrgs) }] } });
      onRefresh("github");
      flash("GitHub settings saved");
    } catch (e) {
      flash(`Error: ${e}`);
    }
  }

  async function saveSlack() {
    try {
      const label = slackLabel.trim() || "default";
      if (slackToken.trim()) {
        await invoke("set_secret", { connector: "slack", label, value: slackToken.trim() });
        setSlackToken("");
      }
      await persist({ slack: { workspaces: [{ label }] } });
      onRefresh("slack");
      flash("Slack settings saved");
    } catch (e) {
      flash(`Error: ${e}`);
    }
  }

  return (
    <div className="panels settings">
      {savedMsg && <div className="banner ok-banner">{savedMsg}</div>}

      <section className="card">
        <h2>General</h2>
        <div className="field">
          <label htmlFor="tz">Timezone (IANA)</label>
          <input
            id="tz"
            value={timezone}
            onChange={(e) => setTimezone(e.target.value)}
            placeholder="Asia/Kolkata"
          />
        </div>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={filterBots}
            onChange={(e) => setFilterBots(e.target.checked)}
          />
          Filter bot authors (dependabot and similar)
        </label>
        <div className="field-actions">
          <button className="save-btn" onClick={saveGeneral}>
            Save
          </button>
        </div>
      </section>

      <section className="card">
        <h2>GitHub</h2>
        <div className="field">
          <label htmlFor="gh-label">Account label</label>
          <input id="gh-label" value={ghLabel} onChange={(e) => setGhLabel(e.target.value)} />
        </div>
        <div className="field">
          <label htmlFor="gh-token">Personal access token</label>
          <input
            id="gh-token"
            type="password"
            value={ghToken}
            onChange={(e) => setGhToken(e.target.value)}
            placeholder="Paste to set or replace - stored in the OS keychain"
            autoComplete="off"
          />
        </div>
        <div className="field">
          <label htmlFor="gh-orgs">Organizations</label>
          <input
            id="gh-orgs"
            value={ghOrgs}
            onChange={(e) => setGhOrgs(e.target.value)}
            placeholder="z-roworld, another-org"
          />
        </div>
        <div className="field-actions">
          <button className="save-btn" onClick={saveGithub}>
            Save
          </button>
        </div>
      </section>

      <section className="card">
        <h2>Slack</h2>
        <div className="field">
          <label htmlFor="slack-label">Workspace label</label>
          <input
            id="slack-label"
            value={slackLabel}
            onChange={(e) => setSlackLabel(e.target.value)}
          />
        </div>
        <div className="field">
          <label htmlFor="slack-token">User OAuth token (xoxp)</label>
          <input
            id="slack-token"
            type="password"
            value={slackToken}
            onChange={(e) => setSlackToken(e.target.value)}
            placeholder="Paste to set or replace - stored in the OS keychain"
            autoComplete="off"
          />
        </div>
        <div className="field-actions">
          <button className="save-btn" onClick={saveSlack}>
            Save
          </button>
        </div>
      </section>
    </div>
  );
}
