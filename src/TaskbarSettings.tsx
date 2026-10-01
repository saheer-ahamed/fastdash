import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getConfig, patchConfig } from "./config";
import { t } from "./i18n";
import type { TaskbarConfig, TaskbarMetric } from "./types";

// Settings for the Windows taskbar readout (`src-tauri/src/taskbar/`): whether
// it shows, and which line each number sits on. Every change is saved and
// redrawn at once, like the theme and language switches above it - the readout
// is on screen the whole time, so the taskbar itself is the preview.

/// Every metric, in the order they appear within a line.
const METRICS: { id: TaskbarMetric; label: () => string }[] = [
  { id: "claudeSession", label: () => t("settings.metricClaudeSession") },
  { id: "claudeWeekly", label: () => t("settings.metricClaudeWeekly") },
  { id: "githubOpened", label: () => t("settings.metricGithubOpened") },
  { id: "githubMerged", label: () => t("settings.metricGithubMerged") },
];

/// Where a metric goes: line 0 (top), line 1 (bottom), or nowhere.
type Slot = 0 | 1 | null;

const SLOTS: { id: Slot; label: () => string }[] = [
  { id: 0, label: () => t("settings.taskbarTop") },
  { id: 1, label: () => t("settings.taskbarBottom") },
  { id: null, label: () => t("settings.taskbarOff") },
];

function slotOf(lines: TaskbarMetric[][], metric: TaskbarMetric): Slot {
  if (lines[0]?.includes(metric)) return 0;
  if (lines[1]?.includes(metric)) return 1;
  return null;
}

/// Rebuild the two lines with `metric` moved to `slot`. Within a line the order
/// is always `METRICS` order, so the readout reads the same however it was set.
function moveMetric(lines: TaskbarMetric[][], metric: TaskbarMetric, slot: Slot): TaskbarMetric[][] {
  const slotFor = (m: TaskbarMetric) => (m === metric ? slot : slotOf(lines, m));
  return [0, 1].map((line) => METRICS.map((m) => m.id).filter((m) => slotFor(m) === line));
}

/// The readout is drawn into the Windows taskbar only, so elsewhere there is
/// nothing to configure.
const SUPPORTED = navigator.userAgent.includes("Windows");

export default function TaskbarSettings({ error }: { error: (e: unknown) => void }) {
  const [settings, setSettings] = useState<TaskbarConfig | null>(null);
  // Saves run one after another, so two quick clicks cannot interleave their
  // read-modify-write of the config file and lose the first.
  const queue = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    if (!SUPPORTED) return;
    getConfig()
      .then((cfg) => setSettings(cfg.taskbar))
      .catch(error);
  }, [error]);

  if (!SUPPORTED || !settings) return null;

  function apply(next: TaskbarConfig) {
    setSettings(next);
    queue.current = queue.current
      .then(() => patchConfig({ taskbar: next }))
      .then(() => invoke<void>("taskbar_refresh"))
      .catch(error);
  }

  return (
    <section className="card">
      <h2>{t("settings.taskbar")}</h2>
      <label className="checkbox">
        <input
          type="checkbox"
          checked={settings.enabled}
          onChange={(e) => apply({ ...settings, enabled: e.target.checked })}
        />
        {t("settings.taskbarShow")}
      </label>
      <p className="muted taskbar-lede">{t("settings.taskbarLede")}</p>
      <div className={"metric-rows" + (settings.enabled ? "" : " disabled")}>
        {METRICS.map((metric) => {
          const current = slotOf(settings.lines, metric.id);
          return (
            <div className="metric-row" key={metric.id}>
              <span>{metric.label()}</span>
              <div className="segmented" role="radiogroup" aria-label={metric.label()}>
                {SLOTS.map((slot) => (
                  <button
                    key={String(slot.id)}
                    type="button"
                    role="radio"
                    aria-checked={current === slot.id}
                    disabled={!settings.enabled}
                    className={"seg" + (current === slot.id ? " active" : "")}
                    onClick={() =>
                      apply({
                        ...settings,
                        lines: moveMetric(settings.lines, metric.id, slot.id),
                      })
                    }
                  >
                    {slot.label()}
                  </button>
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
