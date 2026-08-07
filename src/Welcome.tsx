import { CONNECTOR_TABS } from "./connectors";
import { t } from "./i18n";

// The first run: what fills the content pane while nothing is connected, so the
// app opens on a way in rather than on an empty dashboard.
//
// It is a router, not a second onboarding. Each card hands off to that
// connector's own setup form, where whatever that connector already does for a
// newcomer - GitHub's walkthrough, Claude's Console explainer - happens as
// usual; duplicating any of it here would mean two places to keep true.
//
// Registry-driven off CONNECTOR_TABS, and deliberately not gated on a "seen"
// flag: the condition that shows this page is "nothing is connected", which
// heals itself the moment something is. A dismissible flag would instead strand
// whoever dismissed it on a blank dashboard with no way back.
export default function Welcome({ onConnect }: { onConnect: (id: string) => void }) {
  return (
    <>
      <header className="topbar">
        <h1>{t("welcome.title")}</h1>
      </header>

      <div className="panels">
        <p className="welcome-lede">{t("welcome.lede")}</p>

        {CONNECTOR_TABS.length === 0 ? (
          <div className="empty">{t("app.noConnectors")}</div>
        ) : (
          <>
            <div className="section-heading">
              <h2>{t("welcome.connectors")}</h2>
            </div>

            <div className="welcome-grid">
              {CONNECTOR_TABS.map((c) => (
                <section key={c.id} className="card connector-card">
                  <h2>{t(c.labelKey)}</h2>
                  <p className="connector-blurb">{t(c.blurbKey)}</p>
                  <div className="field-actions">
                    {/* Every button reads "Connect", so the accessible name has
                        to carry which connector it belongs to. */}
                    <button
                      type="button"
                      className="save-btn"
                      aria-label={t("welcome.connectAria", { name: t(c.labelKey) })}
                      onClick={() => onConnect(c.id)}
                    >
                      {t("welcome.connect")}
                    </button>
                  </div>
                </section>
              ))}
            </div>

            <p className="muted welcome-note">{t("welcome.footnote")}</p>
          </>
        )}
      </div>
    </>
  );
}
