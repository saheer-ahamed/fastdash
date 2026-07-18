import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ConnectorMeta, Health, Panel, Snapshot } from "./types";

export default function App() {
  const [connectors, setConnectors] = useState<ConnectorMeta[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [snapshots, setSnapshots] = useState<Record<string, Snapshot>>({});
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    invoke<ConnectorMeta[]>("list_connectors")
      .then((cs) => {
        setConnectors(cs);
        if (cs.length > 0) setActive(cs[0].id);
      })
      .catch((e) => console.error(e));
  }, []);

  const refresh = useCallback((id: string) => {
    setLoading(true);
    invoke<Snapshot>("fetch_connector", { id })
      .then((snap) => setSnapshots((s) => ({ ...s, [id]: snap })))
      .catch((e) => console.error(e))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    if (active) refresh(active);
  }, [active, refresh]);

  const snap = active ? snapshots[active] : undefined;
  const activeName = connectors.find((c) => c.id === active)?.name ?? "";

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">fastdash</div>
        <nav>
          {connectors.map((c) => (
            <button
              key={c.id}
              className={"tab" + (c.id === active ? " active" : "")}
              onClick={() => setActive(c.id)}
            >
              <span className={"dot " + statusClass(snapshots[c.id]?.status)} />
              {c.name}
            </button>
          ))}
        </nav>
      </aside>

      <main className="content">
        <header className="topbar">
          <h1>{activeName}</h1>
          <div className="actions">
            {snap && <span className="muted">updated {fetchedLabel(snap.fetchedAt)}</span>}
            <button
              className="refresh"
              disabled={loading || !active}
              onClick={() => active && refresh(active)}
            >
              {loading ? "..." : "Refresh"}
            </button>
          </div>
        </header>

        {snap ? <SnapshotView snapshot={snap} /> : <div className="empty">Loading...</div>}
      </main>
    </div>
  );
}

function SnapshotView({ snapshot }: { snapshot: Snapshot }) {
  return (
    <div className="panels">
      <StatusBanner status={snapshot.status} />
      {snapshot.panels.map((panel, i) => (
        <PanelView key={i} panel={panel} />
      ))}
    </div>
  );
}

function StatusBanner({ status }: { status: Health }) {
  if (status.state === "ok") return null;
  const text =
    status.state === "needsAuth"
      ? status.message
      : status.state === "error"
        ? status.message
        : "Rate limited - retrying shortly";
  return <div className={"banner " + statusClass(status)}>{text}</div>;
}

function PanelView({ panel }: { panel: Panel }) {
  switch (panel.kind) {
    case "statCards":
      return (
        <section className="card">
          {panel.title && <h2>{panel.title}</h2>}
          <div className="stat-grid">
            {panel.stats.map((s, i) => (
              <div key={i} className="stat">
                <div className="stat-value">{s.value}</div>
                <div className="stat-label">{s.label}</div>
                {s.sub && <div className="stat-sub">{s.sub}</div>}
              </div>
            ))}
          </div>
        </section>
      );
    case "meter": {
      const pct = panel.limit ? Math.min(100, (panel.used / panel.limit) * 100) : 0;
      return (
        <section className="card">
          <div className="meter-head">
            <span>{panel.label}</span>
            <span className="muted">{panel.caption ?? `${Math.round(pct)}%`}</span>
          </div>
          <div className="meter-track">
            <div className="meter-fill" style={{ width: `${pct}%` }} />
          </div>
        </section>
      );
    }
    case "table":
      return (
        <section className="card">
          {panel.title && <h2>{panel.title}</h2>}
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  {panel.columns.map((col) => (
                    <th key={col.key} className={col.numeric ? "num" : ""}>
                      {col.label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {panel.rows.map((row, ri) => (
                  <tr key={ri}>
                    {row.map((cell, ci) => (
                      <td key={ci} className={panel.columns[ci]?.numeric ? "num" : ""}>
                        {cell.href ? (
                          <a href={cell.href} target="_blank" rel="noreferrer">
                            {cell.text}
                          </a>
                        ) : (
                          cell.text
                        )}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      );
    case "barList":
      return (
        <section className="card">
          {panel.title && <h2>{panel.title}</h2>}
          <div className="bars">
            {panel.bars.map((b, i) => (
              <div key={i} className="bar-row">
                <span className="bar-label">{b.label}</span>
                <span className="bar-track">
                  <span className="bar-fill" style={{ width: `${Math.min(100, b.value * 100)}%` }} />
                </span>
                <span className="bar-value muted">{b.display ?? ""}</span>
              </div>
            ))}
          </div>
        </section>
      );
    case "list":
      return (
        <section className="card">
          {panel.title && <h2>{panel.title}</h2>}
          <ul className="list">
            {panel.items.map((item, i) => (
              <li key={i}>
                <div className="list-main">
                  {item.href ? (
                    <a href={item.href} target="_blank" rel="noreferrer">
                      {item.title}
                    </a>
                  ) : (
                    <span>{item.title}</span>
                  )}
                  {item.subtitle && <span className="muted"> {item.subtitle}</span>}
                </div>
                {item.meta && <span className="muted">{item.meta}</span>}
              </li>
            ))}
          </ul>
        </section>
      );
  }
}

function statusClass(status: Health | undefined): string {
  switch (status?.state) {
    case "ok":
      return "ok";
    case "needsAuth":
    case "rateLimited":
      return "warn";
    case "error":
      return "err";
    default:
      return "idle";
  }
}

function fetchedLabel(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}
