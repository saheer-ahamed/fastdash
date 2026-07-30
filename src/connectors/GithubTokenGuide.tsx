import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";

// The "how do I make a token?" walkthrough, opened from the small (i) beside the
// token box. It is a modal rather than a hover tooltip on purpose: this is seven
// steps of instructions you follow with GitHub open in another window, so it has
// to stay put while you look away, and it has to be readable on a phone-sized
// window and by a screen reader. Hovering can't do any of that.
//
// Every string lives in the catalog (locales/en/app.json, section `patGuide`) and
// is deliberately jargon-free - the reader is being asked to do something on a
// site they may never have opened the settings of before.

export default function GithubTokenGuide({
  docsUrl,
  onClose,
}: {
  docsUrl: string;
  onClose: () => void;
}) {
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    closeRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const scopes = [
    { name: "repo", why: t("patGuide.scopeRepo") },
    { name: "read:org", why: t("patGuide.scopeOrg") },
    { name: "read:user", why: t("patGuide.scopeUser") },
  ];

  return createPortal(
    // Clicking the dimmed backdrop closes; the keyboard route is Escape, which
    // is why the scrim needs no role of its own.
    <div className="modal-scrim" onClick={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="token-guide-title"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2 id="token-guide-title">{t("patGuide.title")}</h2>
          <button type="button" className="link-btn" ref={closeRef} onClick={onClose}>
            {t("patGuide.close")}
          </button>
        </div>

        <div className="modal-body">
          <p className="guide-lede">{t("patGuide.lede")}</p>

          <ol className="guide-steps">
            <li>
              <strong>{t("patGuide.s1Title")}</strong>
              {t("patGuide.s1Body")}
            </li>
            <li>
              <strong>{t("patGuide.s2Title")}</strong>
              {t("patGuide.s2Body")}
            </li>
            <li>
              <strong>{t("patGuide.s3Title")}</strong>
              {t("patGuide.s3Body")}
            </li>
            <li>
              <strong>{t("patGuide.s4Title")}</strong>
              {t("patGuide.s4Body")}
            </li>
            <li>
              <strong>{t("patGuide.s5Title")}</strong>
              {t("patGuide.s5Body")}
              <ul className="guide-scopes">
                {scopes.map((s) => (
                  <li key={s.name}>
                    <code>{s.name}</code>
                    <span>{s.why}</span>
                  </li>
                ))}
              </ul>
            </li>
            <li>
              <strong>{t("patGuide.s6Title")}</strong>
              {t("patGuide.s6Body")}
            </li>
            <li>
              <strong>{t("patGuide.s7Title")}</strong>
              {t("patGuide.s7Body")}
            </li>
          </ol>

          <div className="guide-note">
            <strong>{t("patGuide.fineGrainedTitle")}</strong>
            <p>{t("patGuide.fineGrainedBody")}</p>
          </div>

          <div className="guide-note">
            <strong>{t("patGuide.safetyTitle")}</strong>
            <p>{t("patGuide.safetyBody")}</p>
          </div>

          <div className="field-actions">
            <a
              className="link-btn"
              href={docsUrl}
              onClick={(e) => {
                e.preventDefault();
                invoke("open_external", { url: docsUrl }).catch(() => {});
              }}
            >
              {t("patGuide.docs")}
            </a>
          </div>
        </div>
      </div>
    </div>,
    document.body,
  );
}
