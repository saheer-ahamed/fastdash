import { useEffect, useState } from "react";
import type { DateRange } from "./types";
import { PRESETS, presetRange, toISODate, type PresetId } from "./range";
import { t } from "./i18n";

// The date filter shown above every dashboard: preset chips plus a custom
// start/end pair. Presets resolve to concrete dates here, so a connector only
// ever receives a span - see `src/range.ts`.
export default function RangeFilter({
  range,
  preset,
  onChange,
}: {
  range: DateRange;
  preset: PresetId;
  onChange: (next: DateRange, preset: PresetId) => void;
}) {
  // Draft custom dates, so a half-typed range never triggers a fetch.
  const [draft, setDraft] = useState<DateRange>(range);
  const today = toISODate(new Date());

  // Seed the custom inputs from whatever is selected when the panel opens.
  useEffect(() => {
    if (preset === "custom") setDraft(range);
  }, [preset, range]);

  const applyDisabled = !draft.start || !draft.end;

  return (
    <div className="range-filter">
      <span className="range-label muted">{t("range.label")}</span>
      {/* Chips and the custom row share a column so the second row lines up
          under the first instead of under the label. */}
      <div className="range-body">
        <div className="range-chips">
          {PRESETS.map((p) => (
            <button
              key={p.id}
              className={"chip" + (p.id === preset ? " active" : "")}
              aria-pressed={p.id === preset}
              onClick={() => onChange(presetRange(p.id, range), p.id)}
            >
              {t(p.labelKey)}
            </button>
          ))}
        </div>

        {preset === "custom" && (
          <div className="range-custom">
            <label>
              <span className="muted">{t("range.from")}</span>
              <input
                type="date"
                max={draft.end || today}
                value={draft.start}
                onChange={(e) => setDraft((d) => ({ ...d, start: e.target.value }))}
              />
            </label>
            <label>
              <span className="muted">{t("range.to")}</span>
              <input
                type="date"
                min={draft.start}
                max={today}
                value={draft.end}
                onChange={(e) => setDraft((d) => ({ ...d, end: e.target.value }))}
              />
            </label>
            <button
              className="save-btn"
              disabled={applyDisabled}
              onClick={() => onChange(draft, "custom")}
            >
              {t("range.apply")}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
