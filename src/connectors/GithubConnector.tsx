import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { GithubAccount } from "../types";
import { getConfig, patchConfig } from "../config";
import { t } from "../i18n";
import type { ConnectorTabProps } from "./types";

// GitHub connector setup: add/remove accounts, per-account token (Device Flow or
// pasted PAT), and the orgs to track. Tokens go straight to the OS keychain via
// `set_secret` and are never read back; only labels + orgs are persisted to the
// config, and only the `github` slice of it (see `patchConfig`).

/// GitHub Device Flow codes returned by `github_device_start`.
type DeviceCode = {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  expiresIn: number;
  interval: number;
};

const parseOrgs = (raw: string): string[] =>
  raw
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter(Boolean);

// One editable row: keeps orgs as a raw string while typing; `key` is stable so
// per-row token/device state survives reordering and removals.
type AccountRow = { key: number; label: string; orgs: string };

export default function GithubConnector({ onRefresh, flash, error }: ConnectorTabProps) {
  const nextKey = useRef(0);
  const [rows, setRows] = useState<AccountRow[]>([]);
  // Newly typed tokens, per row key (only sent to the keychain on save).
  const [tokenInput, setTokenInput] = useState<Record<number, string>>({});
  const [replacing, setReplacing] = useState<Record<number, boolean>>({});
  // Whether a token exists in the keychain, per account label.
  const [stored, setStored] = useState<Record<string, boolean>>({});
  // The single in-flight Device Flow login, tagged with its row key.
  const [connect, setConnect] = useState<{
    key: number;
    device: DeviceCode | null;
    copied: boolean;
  } | null>(null);
  const connectId = useRef(0);

  // Seed the rows from the saved accounts on mount.
  useEffect(() => {
    let cancelled = false;
    getConfig()
      .then((cfg) => {
        if (cancelled) return;
        setRows(
          cfg.github.accounts.map((a) => ({
            key: nextKey.current++,
            label: a.label,
            orgs: a.orgs.join(", "),
          })),
        );
      })
      .catch((e) => console.error(e));
    return () => {
      cancelled = true;
    };
  }, []);

  // Refresh the "token stored" flags whenever the set of labels changes. We only
  // ever ask whether a secret exists, never read it back. NUL can't appear in a
  // label, so it separates them without two labels ever colliding.
  const labelsKey = rows.map((r) => r.label.trim()).join("\u0000");
  useEffect(() => {
    let cancelled = false;
    const labels = Array.from(new Set(rows.map((r) => r.label.trim()).filter(Boolean)));
    Promise.all(labels.map(async (label) => [label, await hasSecret(label)] as const))
      .then((pairs) => {
        if (!cancelled) setStored(Object.fromEntries(pairs));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [labelsKey]);

  async function hasSecret(label: string): Promise<boolean> {
    try {
      return await invoke<boolean>("has_secret", { connector: "github", label });
    } catch {
      return false;
    }
  }

  // Persist only this connector's slice of the config.
  const saveAccounts = (accounts: GithubAccount[]) => patchConfig({ github: { accounts } });

  function patchRow(key: number, patch: Partial<AccountRow>) {
    setRows((rs) => rs.map((r) => (r.key === key ? { ...r, ...patch } : r)));
  }

  function addAccount() {
    setRows((rs) => [...rs, { key: nextKey.current++, label: "", orgs: "" }]);
  }

  // Rows -> persisted accounts, dropping any without a label.
  function toAccounts(rs: AccountRow[]): GithubAccount[] {
    return rs
      .map((r) => ({ label: r.label.trim(), orgs: parseOrgs(r.orgs) }))
      .filter((a) => a.label);
  }

  async function removeAccount(key: number) {
    const row = rows.find((r) => r.key === key);
    const remaining = rows.filter((r) => r.key !== key);
    setRows(remaining);
    try {
      if (row?.label.trim()) {
        await invoke("delete_secret", { connector: "github", label: row.label.trim() }).catch(
          () => {},
        );
      }
      await saveAccounts(toAccounts(remaining));
      onRefresh();
      flash(t("settings.saved", { section: t("settings.github") }));
    } catch (e) {
      error(e);
    }
  }

  async function saveAll() {
    try {
      // Push any freshly typed tokens to the keychain first, keyed by label.
      for (const r of rows) {
        const token = tokenInput[r.key]?.trim();
        const label = r.label.trim();
        if (token && label) {
          await invoke("set_secret", { connector: "github", label, value: token });
        }
      }
      const justStored = rows.filter((r) => tokenInput[r.key]?.trim() && r.label.trim());
      setTokenInput({});
      setReplacing({});
      await saveAccounts(toAccounts(rows));
      if (justStored.length) {
        setStored((s) => {
          const next = { ...s };
          for (const r of justStored) next[r.label.trim()] = true;
          return next;
        });
      }
      onRefresh();
      flash(t("settings.saved", { section: t("settings.github") }));
    } catch (e) {
      error(e);
    }
  }

  // Device Flow for one row: fetch a code, show it while the user approves in the
  // browser, then persist the account so the connector picks it up.
  async function connectAccount(key: number) {
    const label = rows.find((r) => r.key === key)?.label.trim();
    if (!label) {
      error(new Error("Set an account label first"));
      return;
    }
    const id = ++connectId.current;
    setConnect({ key, device: null, copied: false });
    try {
      const dc = await invoke<DeviceCode>("github_device_start");
      if (connectId.current !== id) return; // cancelled while starting
      setConnect({ key, device: dc, copied: false });
      const login = await invoke<string>("github_device_poll", {
        deviceCode: dc.deviceCode,
        interval: dc.interval,
        label,
      });
      if (connectId.current !== id) return; // cancelled while polling
      setConnect(null);
      setStored((s) => ({ ...s, [label]: true }));
      setReplacing((r) => ({ ...r, [key]: false }));
      await saveAccounts(toAccounts(rows));
      onRefresh();
      flash(t("settings.connectedAs", { login }));
    } catch (e) {
      if (connectId.current === id) {
        setConnect(null);
        error(e);
      }
    }
  }

  function cancelConnect() {
    connectId.current++; // invalidate the in-flight poll
    setConnect(null);
  }

  async function copyCode(code: string) {
    try {
      await navigator.clipboard.writeText(code);
      setConnect((c) => (c ? { ...c, copied: true } : c));
      window.setTimeout(() => setConnect((c) => (c ? { ...c, copied: false } : c)), 1500);
    } catch {
      /* clipboard may be unavailable; the code is still shown to type manually */
    }
  }

  return (
    // The active sub-tab already names the connector, so the card carries no
    // heading of its own.
    <section className="card">
      {rows.length === 0 && <p className="muted">{t("settings.noGithubAccounts")}</p>}

      {rows.map((row, i) => {
        const label = row.label.trim();
        const tokenStored = !!label && !!stored[label];
        const isReplacing = !!replacing[row.key];
        const connecting = connect?.key === row.key;
        const device = connecting ? connect?.device : null;
        return (
          <div className="account-card" key={row.key}>
            <div className="account-head">
              <span className="muted">{t("settings.accountN", { n: i + 1 })}</span>
              <button type="button" className="link-btn" onClick={() => removeAccount(row.key)}>
                {t("settings.removeAccount")}
              </button>
            </div>

            <div className="field">
              <label>{t("settings.accountLabel")}</label>
              <input
                value={row.label}
                onChange={(e) => patchRow(row.key, { label: e.target.value })}
              />
            </div>

            {device ? (
              <div className="device-flow">
                <p>{t("settings.deviceInstructions")}</p>
                <div className="device-code">
                  <code>{device.userCode}</code>
                  <button
                    type="button"
                    className="link-btn"
                    onClick={() => copyCode(device.userCode)}
                  >
                    {connect?.copied ? t("settings.copied") : t("settings.copy")}
                  </button>
                </div>
                <p className="muted device-waiting">{t("settings.deviceWaiting")}</p>
                <div className="field-actions">
                  <a
                    className="link-btn"
                    href={device.verificationUri}
                    onClick={(e) => {
                      e.preventDefault();
                      invoke("open_external", { url: device.verificationUri }).catch(() => {});
                    }}
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
                  <button
                    className="save-btn"
                    onClick={() => connectAccount(row.key)}
                    disabled={connecting}
                  >
                    {connecting ? t("settings.connecting") : t("settings.connectGithub")}
                  </button>
                </div>

                <div className="field">
                  <label>{tokenStored ? t("settings.orPasteToken") : t("settings.pat")}</label>
                  {tokenStored && !isReplacing ? (
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
                          setReplacing((r) => ({ ...r, [row.key]: true }));
                          setTokenInput((ti) => ({ ...ti, [row.key]: "" }));
                        }}
                      >
                        {t("settings.replace")}
                      </button>
                    </div>
                  ) : (
                    <input
                      type="password"
                      value={tokenInput[row.key] ?? ""}
                      onChange={(e) =>
                        setTokenInput((ti) => ({ ...ti, [row.key]: e.target.value }))
                      }
                      placeholder={t("settings.tokenPlaceholder")}
                      autoComplete="off"
                    />
                  )}
                  {tokenStored && (
                    <span className="muted stored-hint">{t("settings.tokenStored")}</span>
                  )}
                </div>
              </>
            )}

            <div className="field">
              <label>{t("settings.orgs")}</label>
              <input
                value={row.orgs}
                onChange={(e) => patchRow(row.key, { orgs: e.target.value })}
                placeholder={t("settings.orgsPlaceholder")}
              />
            </div>
          </div>
        );
      })}

      <div className="field-actions account-actions">
        <button className="save-btn" onClick={addAccount}>
          {t("settings.addAccount")}
        </button>
        <button className="save-btn" onClick={saveAll}>
          {t("settings.save")}
        </button>
      </div>
    </section>
  );
}
