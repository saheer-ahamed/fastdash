import { useEffect, type ReactNode } from "react";

// Retires the boot loader in index.html once React has committed its first
// frame, so the handover happens on a painted UI rather than on a timer.
//
// The loader exists because the window is on screen well before the app is: the
// stylesheet and the JS bundle both arrive after the document, and on a cold
// start (the first launch after an install or an update) that gap is long
// enough to read as a hung app. index.html paints the themed background and
// this loader immediately; everything here does is take it away again.
//
// Mount this *outside* the ErrorBoundary: a crash on the very first render
// still has to lift the loader, or the crash screen stays hidden behind it.
export default function BootGate({ children }: { children: ReactNode }) {
  useEffect(() => {
    document.documentElement.classList.add("booted");
    const boot = document.getElementById("boot");
    // Let the fade finish before taking it out of the document.
    if (boot) window.setTimeout(() => boot.remove(), 200);
  }, []);
  return <>{children}</>;
}
