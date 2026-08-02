import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getConfig, patchConfig } from "../config";
import { t } from "../i18n";
import type { ConnectorTabProps } from "./types";

// Claude connector setup: connect Anthropic Console with an Admin API key.
//
// There is no "Sign in with Claude" button here, and that is not an omission.
// Anthropic runs no third-party OAuth client registration, so a browser
// authorize flow would have to present Claude Code's own client id as ours, and
// since February 2026 their policy reserves subscription OAuth for Claude Code
// and claude.ai. The Admin key is the sanctioned path. The copy below says so
// rather than leaving the missing button looking like an unfinished feature.
//
// The key goes straight to the OS keychain via `claude_connect`, which verifies
// it before storing, and is never read back. Only the organization name is
// persisted to the config, and only the `claude` slice of it.

/// Where the user provisions the key. Opened in their real browser, never in
/// the app's webview - it is an authenticated Anthropic session.
const ADMIN_KEYS_URL = "https://platform.claude.com/settings/admin-keys";

/// The keychain label the backend stores under (`claude/console`), mirroring
/// `connectors::claude::CONSOLE_LABEL`.
const CONSOLE_LABEL = "console";

export default function ClaudeConnector({ onRefresh, flash, error }: ConnectorTabProps) {
  // Whether a key is in the keychain, and which org it belongs to.
  const [stored, setStored] = useState(false);
  const [org, setOrg] = useState("");
  const [keyInput, setKeyInput] = useState("");
  const [replacing, setReplacing] = useState(false);
  const [connecting, setConnecting] = useState(false);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      getConfig().catch(() => null),
      invoke<boolean>("has_secret", { connector: "claude", label: CONSOLE_LABEL }).catch(
        () => false,
      ),
    ])
      .then(([cfg, has]) => {
        if (cancelled) return;
        setOrg(cfg?.claude?.consoleOrg ?? "");
        setStored(has);
      })
      .catch((e) => console.error(e));
    return () => {
      cancelled = true;
    };
  }, []);

  function openAdminKeys() {
    invoke("open_external", { url: ADMIN_KEYS_URL }).catch(() => {});
  }

  async function connect() {
    const key = keyInput.trim();
    if (!key) {
      error(new Error(t("settings.claudeNeedKey")));
      return;
    }
    setConnecting(true);
    try {
      // Verifies against Anthropic before anything is written to the keychain,
      // so a bad key fails here rather than as a broken dashboard later.
      const orgName = await invoke<string>("claude_connect", { key });
      await patchConfig({ claude: { consoleOrg: orgName } });
      setOrg(orgName);
      setStored(true);
      setKeyInput("");
      setReplacing(false);
      onRefresh();
      flash(t("settings.claudeConnected", { org: orgName }));
    } catch (e) {
      error(e);
    } finally {
      setConnecting(false);
    }
  }

  async function disconnect() {
    try {
      await invoke("claude_disconnect");
      await patchConfig({ claude: { consoleOrg: "" } });
      setOrg("");
      setStored(false);
      setKeyInput("");
      setReplacing(false);
      onRefresh();
      flash(t("settings.claudeDisconnected"));
    } catch (e) {
      error(e);
    }
  }

  return (
    <section className="card">
      <div className="account-card">
        <div className="account-head">
          <span className="muted">{t("settings.claudeConsole")}</span>
          {stored && (
            <button type="button" className="link-btn" onClick={disconnect}>
              {t("settings.claudeDisconnect")}
            </button>
          )}
        </div>

        <p className="muted">{t("settings.claudeLede")}</p>

        {stored && !replacing ? (
          <div className="field">
            <label htmlFor="claude-key">{t("settings.claudeKey")}</label>
            <div className="stored-token">
              <input
                id="claude-key"
                type="password"
                value="storedtoken"
                readOnly
                aria-label={t("settings.claudeKeyStored")}
              />
              <button
                type="button"
                className="link-btn"
                onClick={() => {
                  setReplacing(true);
                  setKeyInput("");
                }}
              >
                {t("settings.replace")}
              </button>
            </div>
            <span className="muted stored-hint">
              {org
                ? t("settings.claudeConnectedTo", { org })
                : t("settings.claudeKeyStored")}
            </span>
          </div>
        ) : (
          <>
            <div className="field-actions connect-actions">
              <button className="save-btn" onClick={openAdminKeys}>
                {t("settings.claudeOpenConsole")}
              </button>
            </div>

            <div className="field">
              <label htmlFor="claude-key">{t("settings.claudeKey")}</label>
              <input
                id="claude-key"
                type="password"
                value={keyInput}
                onChange={(e) => setKeyInput(e.target.value)}
                placeholder={t("settings.claudeKeyPlaceholder")}
                autoComplete="off"
              />
              <div className="token-help muted">
                <p>{t("settings.claudeKeySteps")}</p>
                <p>{t("settings.claudeKeyScope")}</p>
              </div>
            </div>

            <div className="field-actions">
              <button className="save-btn" onClick={connect} disabled={connecting}>
                {connecting ? t("settings.connecting") : t("settings.claudeConnect")}
              </button>
              {replacing && (
                <button
                  type="button"
                  className="link-btn"
                  onClick={() => {
                    setReplacing(false);
                    setKeyInput("");
                  }}
                >
                  {t("settings.cancel")}
                </button>
              )}
            </div>
          </>
        )}
      </div>

      {/* Why there is no browser sign-in button, and what still works without
          a key at all. Both are things a user will otherwise wonder about. */}
      <div className="token-help muted">
        <p>{t("settings.claudeNoOauth")}</p>
        <p>{t("settings.claudePlanMeters")}</p>
      </div>
    </section>
  );
}
