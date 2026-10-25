import { useEffect, useState } from "react";

/** The current time, updated every `everyMs` while `running`. */
export function useNow(everyMs: number, running = true): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), everyMs);
    return () => clearInterval(t);
  }, [everyMs, running]);
  return now;
}

/** "3:42 PM": a time of day, as the user's locale writes it. */
export const clock = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { timeStyle: "short" });
