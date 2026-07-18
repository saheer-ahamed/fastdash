import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig } from "./types";
import { THEMES, getStoredTheme, setTheme, type ThemeChoice } from "./theme";
import { t } from "./i18n";

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
  const [theme, setThemeChoice] = useState<ThemeChoice>(getStoredTheme());

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

  const error = (e: unknown) => flash(t("settings.error", { message: String(e) }));

  // Persist the config plus whatever slices the caller overrides in one write,
  // keeping the in-memory `config` state in sync.
  async function persist(patch: Partial<AppConfig>): Promise<AppConfig> {
    const base = config ?? {
      timezone,
      locale: "en",
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
      flash(t("settings.saved", { section: t("settings.general") }));
    } catch (e) {
      error(e);
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
      flash(t("settings.saved", { section: t("settings.github") }));
    } catch (e) {
      error(e);
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
      flash(t("settings.saved", { section: t("settings.slack") }));
    } catch (e) {
      error(e);
    }
  }

  return (
    <div className="panels settings">
      {savedMsg && <div className="banner ok-banner">{savedMsg}</div>}

      <section className="card">
        <h2>{t("settings.general")}</h2>
        <div className="field">
          <label>{t("settings.theme")}</label>
          <div className="segmented">
            {THEMES.map((opt) => (
              <button
                key={opt.id}
                type="button"
                className={"seg" + (theme === opt.id ? " active" : "")}
                onClick={() => {
                  setThemeChoice(opt.id);
                  setTheme(opt.id);
                }}
              >
                {opt.label}
              </button>
            ))}
          </div>
        </div>
        <div className="field">
          <label htmlFor="tz">{t("settings.timezone")}</label>
          <input
            id="tz"
            value={timezone}
            onChange={(e) => setTimezone(e.target.value)}
            placeholder={t("settings.timezonePlaceholder")}
          />
        </div>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={filterBots}
            onChange={(e) => setFilterBots(e.target.checked)}
          />
          {t("settings.filterBots")}
        </label>
        <div className="field-actions">
          <button className="save-btn" onClick={saveGeneral}>
            {t("settings.save")}
          </button>
        </div>
      </section>

      <section className="card">
        <h2>{t("settings.github")}</h2>
        <div className="field">
          <label htmlFor="gh-label">{t("settings.accountLabel")}</label>
          <input id="gh-label" value={ghLabel} onChange={(e) => setGhLabel(e.target.value)} />
        </div>
        <div className="field">
          <label htmlFor="gh-token">{t("settings.pat")}</label>
          <input
            id="gh-token"
            type="password"
            value={ghToken}
            onChange={(e) => setGhToken(e.target.value)}
            placeholder={t("settings.tokenPlaceholder")}
            autoComplete="off"
          />
        </div>
        <div className="field">
          <label htmlFor="gh-orgs">{t("settings.orgs")}</label>
          <input
            id="gh-orgs"
            value={ghOrgs}
            onChange={(e) => setGhOrgs(e.target.value)}
            placeholder={t("settings.orgsPlaceholder")}
          />
        </div>
        <div className="field-actions">
          <button className="save-btn" onClick={saveGithub}>
            {t("settings.save")}
          </button>
        </div>
      </section>

      <section className="card">
        <h2>{t("settings.slack")}</h2>
        <div className="field">
          <label htmlFor="slack-label">{t("settings.workspaceLabel")}</label>
          <input
            id="slack-label"
            value={slackLabel}
            onChange={(e) => setSlackLabel(e.target.value)}
          />
        </div>
        <div className="field">
          <label htmlFor="slack-token">{t("settings.slackToken")}</label>
          <input
            id="slack-token"
            type="password"
            value={slackToken}
            onChange={(e) => setSlackToken(e.target.value)}
            placeholder={t("settings.tokenPlaceholder")}
            autoComplete="off"
          />
        </div>
        <div className="field-actions">
          <button className="save-btn" onClick={saveSlack}>
            {t("settings.save")}
          </button>
        </div>
      </section>
    </div>
  );
}
