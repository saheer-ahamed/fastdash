import { useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { getConfig, patchConfig } from "./config";
import { useFlash } from "./flash";
import { THEMES, getStoredTheme, setTheme, type ThemeChoice } from "./theme";
import { LOCALES, getLocale, setLocale, t } from "./i18n";
import { DEV_MODE_TAPS, isDevMode, setDevMode } from "./devmode";

// App-wide preferences. Connector credentials and per-connector options live on
// the Connectors page instead; this section writes only its own config slice.

export default function Settings({ onLocaleChange }: { onLocaleChange: (locale: string) => void }) {
  const [timezone, setTimezone] = useState("");
  const [filterBots, setFilterBots] = useState(true);
  const [theme, setThemeChoice] = useState<ThemeChoice>(getStoredTheme());
  const [locale, setLocaleChoice] = useState(getLocale());

  const { message, flash, error } = useFlash();

  useEffect(() => {
    getConfig()
      .then((cfg) => {
        setTimezone(cfg.timezone);
        setFilterBots(cfg.filterBots);
        setLocaleChoice(cfg.locale);
        setLocale(cfg.locale);
      })
      .catch((e) => console.error(e));
  }, []);

  async function changeLocale(next: string) {
    setLocaleChoice(next);
    try {
      await patchConfig({ locale: next });
      onLocaleChange(next);
    } catch (e) {
      error(e);
    }
  }

  async function saveGeneral() {
    try {
      await patchConfig({ timezone, filterBots });
      flash(t("settings.saved", { section: t("settings.general") }));
    } catch (e) {
      error(e);
    }
  }

  return (
    <div className="panels settings">
      {message && <div className="banner ok-banner">{message}</div>}

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

      <AboutCard flash={flash} />
    </div>
  );
}

// About + the hidden dev-mode switch: tapping the version line five times flips
// developer mode (mirrors the classic "tap the build number" gesture). A short
// idle window resets the count so stray clicks don't accumulate. Feedback goes
// through the same `flash` banner the rest of Settings uses.
function AboutCard({ flash }: { flash: (msg: string) => void }) {
  const [version, setVersion] = useState("");
  const [devMode, setDevModeState] = useState(isDevMode());
  const taps = useRef(0);
  const resetTimer = useRef<number | null>(null);

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
    return () => {
      if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
    };
  }, []);

  function tapVersion() {
    if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
    resetTimer.current = window.setTimeout(() => {
      taps.current = 0;
    }, 2000);

    taps.current += 1;
    const remaining = DEV_MODE_TAPS - taps.current;

    if (remaining <= 0) {
      taps.current = 0;
      window.clearTimeout(resetTimer.current);
      const next = !devMode;
      setDevMode(next);
      setDevModeState(next);
      flash(next ? t("settings.devModeOn") : t("settings.devModeOff"));
    } else if (remaining <= DEV_MODE_TAPS - 2) {
      // Only start hinting once they're clearly on the way (after 2 taps).
      flash(t("settings.devModeCountdown", { n: remaining }));
    }
  }

  return (
    <section className="card">
      <h2>{t("settings.about")}</h2>
      <div className="field">
        <label>{t("settings.version")}</label>
        <button type="button" className="version-tap" onClick={tapVersion}>
          {version ? `v${version}` : "-"}
        </button>
      </div>
      {devMode && <p className="muted dev-status">{t("settings.devModeActive")}</p>}
    </section>
  );
}