import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig } from "./types";
import { THEMES, getStoredTheme, setTheme, type ThemeChoice } from "./theme";
import { LOCALES, getLocale, setLocale, t } from "./i18n";

// Minimal settings UI: non-secret config goes to `save_config`; tokens go
// straight to the OS keychain via `set_secret` and are never read back.
// After a successful save we ask the parent to refresh that connector so the
// user sees results immediately instead of waiting for the next scheduler tick.

/// GitHub Device Flow codes returned by `github_device_start`.
type DeviceCode = {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  expiresIn: number;
  interval: number;
};

export default function Settings({
  onRefresh,
  onLocaleChange,
}: {
  onRefresh: (connectorId: string) => void;
  onLocaleChange: (locale: string) => void;
}) {
  const [config, setConfig] = useState<AppConfig | null>(null);

  // General
  const [timezone, setTimezone] = useState("");
  const [filterBots, setFilterBots] = useState(true);
  const [theme, setThemeChoice] = useState<ThemeChoice>(getStoredTheme());
  const [locale, setLocaleChoice] = useState(getLocale());

  // GitHub (single account in the minimal UI)
  const [ghLabel, setGhLabel] = useState("work");
  const [ghOrgs, setGhOrgs] = useState("");
  const [ghToken, setGhToken] = useState("");
  const [ghTokenStored, setGhTokenStored] = useState(false);
  const [ghReplacing, setGhReplacing] = useState(false);
  // Device Flow state: the active code pair while a login is in progress.
  const [device, setDevice] = useState<DeviceCode | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [copied, setCopied] = useState(false);
  // Bumped to invalidate an in-flight poll when the user cancels.
  const connectId = useRef(0);

  // Slack (single workspace in the minimal UI)
  const [slackLabel, setSlackLabel] = useState("default");
  const [slackToken, setSlackToken] = useState("");
  const [slackTokenStored, setSlackTokenStored] = useState(false);
  const [slackReplacing, setSlackReplacing] = useState(false);

  const [savedMsg, setSavedMsg] = useState<string | null>(null);

  useEffect(() => {
    invoke<AppConfig>("get_config")
      .then((cfg) => {
        setConfig(cfg);
        setTimezone(cfg.timezone);
        setFilterBots(cfg.filterBots);
        setLocaleChoice(cfg.locale);
        setLocale(cfg.locale);
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

  // Reflect whether a token is already stored for the current label, so the
  // input can show a masked "stored" state instead of looking empty. We never
  // read the secret itself back - only whether one exists.
  useEffect(() => {
    const label = ghLabel.trim() || "work";
    invoke<boolean>("has_secret", { connector: "github", label })
      .then(setGhTokenStored)
      .catch(() => setGhTokenStored(false));
  }, [ghLabel]);

  useEffect(() => {
    const label = slackLabel.trim() || "default";
    invoke<boolean>("has_secret", { connector: "slack", label })
      .then(setSlackTokenStored)
      .catch(() => setSlackTokenStored(false));
  }, [slackLabel]);

  const flash = (msg: string) => {
    setSavedMsg(msg);
    window.setTimeout(() => setSavedMsg(null), 2500);
  };

  const error = (e: unknown) => flash(t("settings.error", { message: String(e) }));

  async function changeLocale(next: string) {
    setLocaleChoice(next);
    try {
      await persist({ locale: next });
      onLocaleChange(next);
    } catch (e) {
      error(e);
    }
  }

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
        setGhTokenStored(true);
        setGhReplacing(false);
      }
      await persist({ github: { accounts: [{ label, orgs: parseOrgs(ghOrgs) }] } });
      onRefresh("github");
      flash(t("settings.saved", { section: t("settings.github") }));
    } catch (e) {
      error(e);
    }
  }

  // Kick off GitHub Device Flow: fetch a code, show it while the user approves
  // in the browser, then persist the account so the connector picks it up.
  async function connectGithub() {
    const label = ghLabel.trim() || "work";
    const id = ++connectId.current;
    setConnecting(true);
    setCopied(false);
    try {
      const dc = await invoke<DeviceCode>("github_device_start");
      if (connectId.current !== id) return; // cancelled while starting
      setDevice(dc);
      const login = await invoke<string>("github_device_poll", {
        deviceCode: dc.deviceCode,
        interval: dc.interval,
        label,
      });
      if (connectId.current !== id) return; // cancelled while polling
      setDevice(null);
      setGhTokenStored(true);
      setGhReplacing(false);
      await persist({ github: { accounts: [{ label, orgs: parseOrgs(ghOrgs) }] } });
      onRefresh("github");
      flash(t("settings.connectedAs", { login }));
    } catch (e) {
      if (connectId.current === id) {
        setDevice(null);
        error(e);
      }
    } finally {
      if (connectId.current === id) setConnecting(false);
    }
  }

  function cancelConnect() {
    connectId.current++; // invalidate the in-flight poll
    setDevice(null);
    setConnecting(false);
  }

  async function copyCode(code: string) {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard may be unavailable; the code is still shown to type manually */
    }
  }

  async function saveSlack() {
    try {
      const label = slackLabel.trim() || "default";
      if (slackToken.trim()) {
        await invoke("set_secret", { connector: "slack", label, value: slackToken.trim() });
        setSlackToken("");
        setSlackTokenStored(true);
        setSlackReplacing(false);
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
          <label>{t("settings.language")}</label>
          <div className="segmented">
            {LOCALES.map((loc) => (
              <button
                key={loc.id}
                type="button"
                className={"seg" + (locale === loc.id ? " active" : "")}
                onClick={() => changeLocale(loc.id)}
              >
                {loc.label}
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

        {device ? (
          <div className="device-flow">
            <p>{t("settings.deviceInstructions")}</p>
            <div className="device-code">
              <code>{device.userCode}</code>
              <button type="button" className="link-btn" onClick={() => copyCode(device.userCode)}>
                {copied ? t("settings.copied") : t("settings.copy")}
              </button>
            </div>
            <p className="muted device-waiting">{t("settings.deviceWaiting")}</p>
            <div className="field-actions">
              <a
                className="link-btn"
                href={device.verificationUri}
                target="_blank"
                rel="noreferrer"
              >
                {t("settings.deviceOpen")}
              </a>
              <button type="button" className="link-btn" onClick={cancelConnect}>
                {t("settings.cancel")}
              </button>
            </div>
          </div>
        ) : (
          <>
            <div className="field-actions connect-actions">
              <button className="save-btn" onClick={connectGithub} disabled={connecting}>
                {connecting ? t("settings.connecting") : t("settings.connectGithub")}
              </button>
            </div>

            <div className="field">
              <label htmlFor="gh-token">
                {ghTokenStored ? t("settings.orPasteToken") : t("settings.pat")}
              </label>
              {ghTokenStored && !ghReplacing ? (
                <div className="stored-token">
                  <input type="password" value="storedtoken" readOnly aria-label={t("settings.tokenStored")} />
                  <button
                    type="button"
                    className="link-btn"
                    onClick={() => {
                      setGhReplacing(true);
                      setGhToken("");
                    }}
                  >
                    {t("settings.replace")}
                  </button>
                </div>
              ) : (
                <input
                  id="gh-token"
                  type="password"
                  value={ghToken}
                  onChange={(e) => setGhToken(e.target.value)}
                  placeholder={t("settings.tokenPlaceholder")}
                  autoComplete="off"
                />
              )}
              {ghTokenStored && <span className="muted stored-hint">{t("settings.tokenStored")}</span>}
            </div>
          </>
        )}

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
          {slackTokenStored && !slackReplacing ? (
            <div className="stored-token">
              <input
                type="password"
                value="storedtoken"
                readOnly
                aria-label={t("settings.tokenStored")}
              />
              <button
                type="button"
                className="link-btn"
                onClick={() => {
                  setSlackReplacing(true);
                  setSlackToken("");
                }}
              >
                {t("settings.replace")}
              </button>
            </div>
          ) : (
            <input
              id="slack-token"
              type="password"
              value={slackToken}
              onChange={(e) => setSlackToken(e.target.value)}
              placeholder={t("settings.tokenPlaceholder")}
              autoComplete="off"
            />
          )}
          {slackTokenStored && (
            <span className="muted stored-hint">{t("settings.tokenStored")}</span>
          )}
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
