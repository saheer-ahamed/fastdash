import { useState } from "react";
import { CONNECTOR_TABS } from "./index";
import { useFlash } from "../flash";
import { t } from "../i18n";

// The Connectors page: one sub-tab per connector, each owning its own form and
// its own Save button. Saving one connector never touches another's settings.

export default function Connectors({ onRefresh }: { onRefresh: (connectorId: string) => void }) {
  const [activeId, setActiveId] = useState(CONNECTOR_TABS[0]?.id ?? "");
  const { message, flash, error } = useFlash();

  const active = CONNECTOR_TABS.find((c) => c.id === activeId) ?? CONNECTOR_TABS[0];

  return (
    <>
      <header className="topbar">
        <h1>{t("app.connectors")}</h1>
      </header>

      <div className="subtabs">
        {CONNECTOR_TABS.map((c) => (
          <button
            key={c.id}
            className={"subtab" + (c.id === active?.id ? " active" : "")}
            onClick={() => setActiveId(c.id)}
          >
            {t(c.labelKey)}
          </button>
        ))}
      </div>

      <div className="panels settings">
        {message && <div className="banner ok-banner">{message}</div>}
        {active ? (
          // Remount on tab switch so each connector form starts from saved state.
          <active.Component
            key={active.id}
            onRefresh={() => onRefresh(active.id)}
            flash={flash}
            error={error}
          />
        ) : (
          <div className="empty">{t("app.noConnectors")}</div>
        )}
      </div>
    </>
  );
}
