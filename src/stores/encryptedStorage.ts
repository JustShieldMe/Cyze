import type { StateStorage } from "zustand/middleware";
import { loadUiState, saveUiState } from "../ipc/commands";

/** Zustand persist storage kept encrypted by the backend, so ceremony history
 *  (groups, recipients, amounts, memos) isn't left in the web view's plaintext
 *  storage. It is readable only while the keystore is unlocked: hydrate after
 *  unlock, and call `lock()` on lock. Writes before a successful load are
 *  dropped, so state from before hydration (or while locked) can never
 *  overwrite the saved history. */
export function encryptedStorage(): StateStorage & { lock: () => void } {
  let loaded = false;
  // Saves are coalesced: one in flight at a time, then the latest value.
  let saving = false;
  let next: { name: string; value: string | null } | null = null;

  const flush = async () => {
    if (saving) return;
    saving = true;
    while (next && loaded) {
      const { name, value } = next;
      next = null;
      await saveUiState(name, value).catch(() => {});
    }
    saving = false;
  };
  const save = (name: string, value: string | null) => {
    if (!loaded) return;
    next = { name, value };
    void flush();
  };

  return {
    getItem: async (name) => {
      const saved = await loadUiState(name);
      loaded = true;
      if (saved !== null) return saved;
      // Earlier versions kept this in localStorage: move it over once.
      let legacy: string | null = null;
      try {
        legacy = localStorage.getItem(name);
        if (legacy !== null) {
          await saveUiState(name, legacy);
          localStorage.removeItem(name);
        }
      } catch {
        // No localStorage, or it can't be read: nothing to move.
      }
      return legacy;
    },
    setItem: (name, value) => save(name, value),
    removeItem: (name) => save(name, null),
    lock: () => {
      loaded = false;
      next = null;
    },
  };
}
