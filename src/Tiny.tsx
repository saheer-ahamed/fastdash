// The minimized widget: a square stuck to the side of the screen.
//
// It is the same window again (see `src-tauri/src/pip.rs`), shrunk past the
// point where anything can be read, so this is deliberately not a view: it is a
// handle. A click opens the widget back up; press and drag slides it up and
// down the edge it is docked to.
//
// The click and the drag share one gesture, so they are told apart the way a
// desktop does it - by distance. Anything under a few pixels of travel was a
// click that wobbled, and anything over it was a drag, which means the click
// only fires on release and never behind a drag that happened to end where it
// started.

import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";

/// How far the pointer may travel before the gesture stops being a click. Three
/// CSS pixels is roughly the wobble of a firm click on a trackpad.
const DRAG_SLOP = 3;

export default function Tiny({ onExpand }: { onExpand: () => void }) {
  const [dragging, setDragging] = useState(false);
  // The gesture in progress. A ref, not state: it changes on every pointer
  // move, and none of it is drawn.
  const gesture = useRef<{
    /** Where the pointer was at the last move, in screen CSS pixels. */
    y: number;
    /** Total travel so far, which is what decides click versus drag. */
    travelled: number;
    /** Physical pixels owed to the window but not yet whole - see below. */
    debt: number;
  } | null>(null);

  const onPointerDown = useCallback((e: React.PointerEvent) => {
    // Only the primary button starts a gesture; a right-click here should do
    // nothing rather than half-open the widget.
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    gesture.current = { y: e.screenY, travelled: 0, debt: 0 };
  }, []);

  const onPointerMove = useCallback((e: React.PointerEvent) => {
    const g = gesture.current;
    if (!g) return;

    const dy = e.screenY - g.y;
    g.y = e.screenY;
    g.travelled += Math.abs(dy);
    if (g.travelled > DRAG_SLOP) setDragging(true);
    else return;

    // The backend moves the window in physical pixels; the pointer reports CSS
    // ones. On a scaled display the conversion has a fraction, and dropping it
    // on every move is a drag that lags visibly behind the cursor - so the
    // remainder is carried into the next move instead.
    const wanted = dy * window.devicePixelRatio + g.debt;
    const whole = Math.trunc(wanted);
    g.debt = wanted - whole;
    if (whole !== 0) void invoke("nudge_tiny", { dy: whole }).catch(() => {});
  }, []);

  const onPointerUp = useCallback(
    (e: React.PointerEvent) => {
      const g = gesture.current;
      gesture.current = null;
      setDragging(false);
      if (!g) return;
      e.currentTarget.releasePointerCapture(e.pointerId);
      if (g.travelled <= DRAG_SLOP) onExpand();
    },
    [onExpand],
  );

  return (
    <button
      className={"tiny" + (dragging ? " dragging" : "")}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      // Keyboard activation only: `detail` is 0 when the click came from Enter
      // or Space rather than a pointer, whose click is already decided above.
      onClick={(e) => {
        if (e.detail === 0) onExpand();
      }}
      // A gesture cancelled by the system (a display change, a lost capture)
      // must not leave the next click looking like the tail of a drag.
      onPointerCancel={() => {
        gesture.current = null;
        setDragging(false);
      }}
      title={t("tiny.hint")}
      aria-label={t("tiny.expand")}
    >
      {/* The app's own icon, so the docked square is recognisably this app and
          not an anonymous handle. It is inert: the square owns the gesture, and
          an image left draggable would let the webview start its own drag in
          the middle of one of ours. */}
      <img className="tiny-mark" src="/favicon.png" alt="" draggable={false} aria-hidden />
    </button>
  );
}
