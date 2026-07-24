// Transient "saved" / "error" banner shared by Settings and each connector tab.

import { useCallback, useEffect, useRef, useState } from "react";
import { t } from "./i18n";

const FLASH_MS = 2500;

export type Flash = {
  message: string | null;
  flash: (msg: string) => void;
  error: (e: unknown) => void;
};

export function useFlash(): Flash {
  const [message, setMessage] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  const flash = useCallback((msg: string) => {
    setMessage(msg);
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setMessage(null), FLASH_MS);
  }, []);

  const error = useCallback(
    (e: unknown) => flash(t("settings.error", { message: String(e) })),
    [flash],
  );

  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    [],
  );

  return { message, flash, error };
}
