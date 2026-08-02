import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SentryAccount } from "../types";
import { getConfig, patchConfig } from "../config";
import { t } from "../i18n";
import type { ConnectorTabProps } from "./types";

// Sentry connector setup: add/remove connections, each with an auth token, the
// Sentry origin (SaaS, a region host, or a self-hosted install), and the
// organizations to report on. Tokens go straight to the OS keychain via
// `set_secret` and are never read back; only labels, URLs and org slugs are
// persisted to the config, and only the `sentry` slice of it (see `patchConfig`).

/// The README section spelling out which token kind and scopes each panel
/// needs; linked from the token field so the answer is one click away.
const TOKEN_DOCS_URL = "https://github.com/saheer-ahamed/fastdash#connecting-sentry";

// Slugs are whitespace-free by construction, so a comma or a space both
// separate entries and neither can appear inside one.
const parseOrgs = (raw: string): string[] => raw.split(/[\s,]+/).filter(Boolean);

// One editable row: keeps orgs as a raw string while typing; `key` is stable so
// per-row token state survives reordering and removals.
type AccountRow = { key: number; label: string; baseUrl: string; orgs: string };

const blankRow = (key: number): AccountRow => ({ key, label: "", baseUrl: "", orgs: "" });

export default function SentryConnector({ onRefresh, flash, error }: ConnectorTabProps) {
  const nextKey = useRef(0);
  const [rows, setRows] = useState<AccountRow[]>([]);
  // Newly typed tokens, per row key (only sent to the keychain on save).
  const [tokenInput, setTokenInput] = useState<Record<number, string>>({});
  const [replacing, setReplacing] = useState<Record<number, boolean>>({});
  // Whether a token exists in the keychain, per connection label.
  const [stored, setStored] = useState<Record<string, boolean>>({});

  // Seed the rows from the saved connections on mount.
  useEffect(() => {
    let cancelled = false;
    getConfig()
      .then((cfg) => {
        if (cancelled) return;
        setRows(
          cfg.sentry.accounts.map((a) => ({
            key: nextKey.current++,
            label: a.label,
            baseUrl: a.baseUrl,
            orgs: a.organizations.join(", "),
          })),
        );
      })
      .catch((e) => console.error(e));
    return () => {
      cancelled = true;
    };
  }, []);

  // Refresh the "token stored" flags whenever the set of labels changes. We only
  // ever ask whether a secret exists, never read it back. A label is free text,
  // so the effect key is the JSON array rather than a joined string: any
  // separator could be typed into a label too, merging two rows into one key.
  const labelsKey = JSON.stringify(rows.map((r) => r.label.trim()));
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
      return await invoke<boolean>("has_secret", { connector: "sentry", label });
    } catch {
      return false;
    }
  }

  // Persist only this connector's slice of the config.
  const saveAccounts = (accounts: SentryAccount[]) => patchConfig({ sentry: { accounts } });

  function patchRow(key: number, patch: Partial<AccountRow>) {
    setRows((rs) => rs.map((r) => (r.key === key ? { ...r, ...patch } : r)));
  }

  function addAccount() {
    setRows((rs) => [...rs, blankRow(nextKey.current++)]);
  }

  // Rows -> persisted connections, dropping any without a label.
  function toAccounts(rs: AccountRow[]): SentryAccount[] {
    return rs
      .map((r) => ({
        label: r.label.trim(),
        baseUrl: r.baseUrl.trim(),
        organizations: parseOrgs(r.orgs),
      }))
      .filter((a) => a.label);
  }

  async function removeAccount(key: number) {
    const row = rows.find((r) => r.key === key);
    const remaining = rows.filter((r) => r.key !== key);
    setRows(remaining);
    try {
      if (row?.label.trim()) {
        await invoke("delete_secret", { connector: "sentry", label: row.label.trim() }).catch(
          () => {},
        );
      }
      await saveAccounts(toAccounts(remaining));
      onRefresh();
      flash(t("settings.saved", { section: t("settings.sentry") }));
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
          await invoke("set_secret", { connector: "sentry", label, value: token });
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
      flash(t("settings.saved", { section: t("settings.sentry") }));
    } catch (e) {
      error(e);
    }
  }

  function openDocs() {
    invoke("open_external", { url: TOKEN_DOCS_URL }).catch(() => {});
  }

  return (
    // The active sub-tab already names the connector, so the card carries no
    // heading of its own.
    <section className="card">
      {rows.length === 0 && <p className="muted">{t("sentry.setup.noAccounts")}</p>}

      {rows.map((row, i) => {
        const label = row.label.trim();
        const tokenStored = !!label && !!stored[label];
        const isReplacing = !!replacing[row.key];
        return (
          <div className="account-card" key={row.key}>
            <div className="account-head">
              <span className="muted">{t("settings.accountN", { n: i + 1 })}</span>
              <button type="button" className="link-btn" onClick={() => removeAccount(row.key)}>
                {t("settings.removeAccount")}
              </button>
            </div>

            <div className="field">
              <label htmlFor={`sentry-label-${row.key}`}>{t("settings.accountLabel")}</label>
              <input
                id={`sentry-label-${row.key}`}
                value={row.label}
                onChange={(e) => patchRow(row.key, { label: e.target.value })}
              />
            </div>

            <div className="field">
              <label htmlFor={`sentry-token-${row.key}`}>{t("sentry.setup.token")}</label>
              {tokenStored && !isReplacing ? (
                <div className="stored-token">
                  <input
                    id={`sentry-token-${row.key}`}
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
                  id={`sentry-token-${row.key}`}
                  type="password"
                  value={tokenInput[row.key] ?? ""}
                  onChange={(e) => setTokenInput((ti) => ({ ...ti, [row.key]: e.target.value }))}
                  placeholder={t("settings.tokenPlaceholder")}
                  autoComplete="off"
                />
              )}
              {tokenStored && <span className="muted stored-hint">{t("settings.tokenStored")}</span>}
              {/* Which scopes to tick, at the moment the token is being made -
                  and which token kind is a dead end, since Sentry offers one
                  that has no issue-reading scope to grant at all. */}
              {(!tokenStored || isReplacing) && (
                <div className="token-help muted">
                  <p>{t("sentry.setup.tokenHelp")}</p>
                  <p>{t("sentry.setup.tokenWarning")}</p>
                  <a
                    className="link-btn"
                    href={TOKEN_DOCS_URL}
                    onClick={(e) => {
                      e.preventDefault();
                      openDocs();
                    }}
                  >
                    {t("sentry.setup.docs")}
                  </a>
                </div>
              )}
            </div>

            <div className="field">
              <label htmlFor={`sentry-url-${row.key}`}>{t("sentry.setup.baseUrl")}</label>
              <input
                id={`sentry-url-${row.key}`}
                value={row.baseUrl}
                onChange={(e) => patchRow(row.key, { baseUrl: e.target.value })}
                placeholder={t("sentry.setup.baseUrlPlaceholder")}
                autoComplete="off"
                spellCheck={false}
              />
              <span className="muted stored-hint">{t("sentry.setup.baseUrlHelp")}</span>
            </div>

            <div className="field">
              <label htmlFor={`sentry-orgs-${row.key}`}>{t("sentry.setup.orgs")}</label>
              <input
                id={`sentry-orgs-${row.key}`}
                value={row.orgs}
                onChange={(e) => patchRow(row.key, { orgs: e.target.value })}
                placeholder={t("sentry.setup.orgsPlaceholder")}
                autoComplete="off"
                spellCheck={false}
              />
              <span className="muted stored-hint">{t("sentry.setup.orgsHelp")}</span>
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
