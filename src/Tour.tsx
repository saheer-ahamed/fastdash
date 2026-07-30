import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { t } from "./i18n";

// A spotlight walkthrough: dims the app, rings one element at a time and
// explains it in a bubble beside it. Purely presentational - the caller owns the
// steps, decides when it opens, and remembers that it ran (see tour.ts).
//
// The dim is one full-screen element with the target punched out of it by an
// even-odd clip-path. That shape has a fixed number of points, so the browser
// interpolates it and the hole glides to the next target on its own; it also
// means the hole is a real hole for the mouse - the highlighted control stays
// clickable and typeable while everything around it is inert. The target's box
// is re-read every frame rather than on scroll/resize events, which keeps the
// ring welded to it while the page smooth-scrolls it into view and while the
// form reflows underneath (rows appear, help text expands, a field collapses).

export type TourStep = {
  /** CSS selector for the element to spotlight; if it matches nothing the bubble centres itself. */
  selector: string;
  title: string;
  body: string;
};

type Box = { top: number; left: number; width: number; height: number };

const PAD = 6; // breathing room between the target and the ring
const GAP = 14; // ring -> bubble
const WIDTH = 340; // bubble width, also what keeps it clear of the viewport edge
const EDGE = 12; // minimum distance from the viewport edge

const sameBox = (a: Box | null, b: Box | null): boolean =>
  a === b ||
  (!!a &&
    !!b &&
    a.top === b.top &&
    a.left === b.left &&
    a.width === b.width &&
    a.height === b.height);

// A full-screen rectangle with `box` punched out of it, as an even-odd polygon:
// the outer ring of points is traced first, then the hole. Every step produces
// the same number of points, which is what lets the browser tween one cutout
// into the next instead of snapping.
const cutout = (box: Box | null): string | undefined => {
  if (!box) return undefined;
  const x1 = Math.round(box.left);
  const y1 = Math.round(box.top);
  const x2 = Math.round(box.left + box.width);
  const y2 = Math.round(box.top + box.height);
  return (
    "polygon(evenodd, 0px 0px, 100% 0px, 100% 100%, 0px 100%, 0px 0px, " +
    `${x1}px ${y1}px, ${x1}px ${y2}px, ${x2}px ${y2}px, ${x2}px ${y1}px, ${x1}px ${y1}px)`
  );
};

const reducedMotion = (): boolean =>
  window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

export default function Tour({ steps, onClose }: { steps: TourStep[]; onClose: () => void }) {
  const [index, setIndex] = useState(0);
  const [box, setBox] = useState<Box | null>(null);
  const [bubbleH, setBubbleH] = useState(0);
  // Nothing is painted until the first measurement lands, so the ring fades in
  // already sitting on its target instead of sweeping in from the top left.
  const [measured, setMeasured] = useState(false);
  const bubbleRef = useRef<HTMLDivElement>(null);
  const nextRef = useRef<HTMLButtonElement>(null);

  const step = steps[index];
  const selector = step?.selector ?? "";
  const last = index >= steps.length - 1;

  // Bring the target into view when the step changes; the frame loop below keeps
  // the ring on it while the scroll animates.
  useEffect(() => {
    document.querySelector(selector)?.scrollIntoView({
      block: "center",
      behavior: reducedMotion() ? "auto" : "smooth",
    });
  }, [selector]);

  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const rect = selector ? document.querySelector(selector)?.getBoundingClientRect() : null;
      const next = rect
        ? {
            top: rect.top - PAD,
            left: rect.left - PAD,
            width: rect.width + PAD * 2,
            height: rect.height + PAD * 2,
          }
        : null;
      // Commit only real changes: once the layout settles the loop is a no-op.
      setBox((prev) => (sameBox(prev, next) ? prev : next));
      const h = bubbleRef.current?.offsetHeight ?? 0;
      setBubbleH((prev) => (prev === h ? prev : h));
      setMeasured(true);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [selector]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      else if (e.key === "ArrowRight") setIndex((i) => Math.min(i + 1, steps.length - 1));
      else if (e.key === "ArrowLeft") setIndex((i) => Math.max(i - 1, 0));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, steps.length]);

  // Hand the keyboard to the walkthrough once it is on screen, so Enter/Space
  // advance it and Tab stays inside the bubble's two or three controls.
  useEffect(() => {
    if (measured) nextRef.current?.focus();
  }, [measured]);

  if (!step || !measured) return null;

  const vw = window.innerWidth;
  const vh = window.innerHeight;
  // Prefer sitting under the target; flip above only when there is room there
  // and not below, so the bubble never hangs off the bottom of the window.
  const fitsBelow = !box || box.top + box.height + GAP + bubbleH + EDGE <= vh;
  const fitsAbove = !!box && box.top - GAP - bubbleH >= EDGE;
  const top = !box
    ? Math.max(EDGE, (vh - bubbleH) / 2)
    : fitsBelow || !fitsAbove
      ? Math.min(box.top + box.height + GAP, Math.max(EDGE, vh - bubbleH - EDGE))
      : box.top - GAP - bubbleH;
  const left = !box
    ? Math.max(EDGE, (vw - WIDTH) / 2)
    : Math.min(Math.max(box.left, EDGE), Math.max(EDGE, vw - WIDTH - EDGE));

  return createPortal(
    <div className="tour" role="dialog" aria-modal="true" aria-labelledby="tour-title">
      {/* Dims and blocks everything except the hole over the current target. */}
      <div className="tour-dim" style={{ clipPath: cutout(box) }} />
      {box && (
        <div
          className="tour-ring"
          style={{
            top: box.top,
            left: box.left,
            width: box.width,
            height: box.height,
          }}
        />
      )}
      <div className="tour-bubble" ref={bubbleRef} style={{ top, left, width: WIDTH }}>
        {/* Keyed by step so each one fades in rather than swapping text in place. */}
        <div className="tour-text" key={index}>
          <span className="muted tour-count">
            {t("tour.step", { n: index + 1, total: steps.length })}
          </span>
          <h3 id="tour-title">{step.title}</h3>
          <p>{step.body}</p>
        </div>
        <div className="tour-actions">
          {!last && (
            <button type="button" className="link-btn" onClick={onClose}>
              {t("tour.skip")}
            </button>
          )}
          <div className="tour-nav">
            {index > 0 && (
              <button type="button" className="link-btn" onClick={() => setIndex(index - 1)}>
                {t("tour.back")}
              </button>
            )}
            <button
              type="button"
              className="save-btn"
              ref={nextRef}
              onClick={() => (last ? onClose() : setIndex(index + 1))}
            >
              {last ? t("tour.done") : t("tour.next")}
            </button>
          </div>
        </div>
      </div>
    </div>,
    document.body,
  );
}
