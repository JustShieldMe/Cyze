import { useQueryClient } from "@tanstack/react-query";
import { useTauriEvent } from "./ipc/events";
import { notifyIfAway, SyncStatus, walletSyncNow } from "./ipc/commands";
import { useCeremonies, CeremonyEventPayload } from "./stores/ceremonies";

/**
 * Subscribes to ceremony progress events at the app root, so updates land in
 * the global store no matter which screen is mounted. Previously the DKG and
 * signing screens each owned their listeners, so navigating away dropped the
 * subscription and the (still-running) backend ceremony appeared to die.
 *
 * Also relays the background wallet sync: `wallet:sync` statuses go into the
 * query cache, and `wallet:updated` re-reads the panels that read the wallet.
 *
 * Renders nothing; mounted once inside the unlocked layout.
 */
export default function CeremonyListener() {
  const queryClient = useQueryClient();
  const { onProgress, onComplete, onFailed } = useCeremonies();

  /**
   * A ceremony just moved funds. Re-read the wallet panels from the local db at
   * once, and have the backend sync so the on-chain transaction is scanned in.
   *
   * This runs for a *signing* completion too, not just a send we coordinated:
   * a participant who approves a transaction never runs the send themselves, so
   * nothing else would ever refresh their wallet — leaving their history openly
   * contradicting the transaction they just signed.
   */
  const refreshWallet = () => {
    for (const key of ["wallet-status", "wallet-history", "wallet-notes"]) {
      queryClient.invalidateQueries({ queryKey: [key] });
    }
    void walletSyncNow(false);
  };

  useTauriEvent<SyncStatus>("wallet:sync", (status) => {
    queryClient.setQueryData(["wallet-sync", status.group_id], status);
  });
  useTauriEvent<{ group_id: string }>("wallet:updated", ({ group_id }) => {
    for (const key of ["wallet-status", "wallet-history", "wallet-notes", "receive-address", "viewing-key"]) {
      queryClient.invalidateQueries({ queryKey: [key, group_id] });
    }
  });

  // Agreeing on a group's private viewing key: its status, and once it's
  // saved, a new address and a backup that's now out of date.
  useTauriEvent<{ group_id: string }>("viewing_key:updated", ({ group_id }) => {
    for (const key of ["viewing-key", "wallet-status", "receive-address", "orchard-keys"]) {
      queryClient.invalidateQueries({ queryKey: [key, group_id] });
    }
    queryClient.invalidateQueries({ queryKey: ["backup-status"] });
  });

  useTauriEvent<CeremonyEventPayload>("dkg:progress", (p) => onProgress("dkg", p));
  useTauriEvent<CeremonyEventPayload>("dkg:complete", (p) => {
    onComplete("dkg", p);
    notifyIfAway("Your new group is ready.");
    queryClient.invalidateQueries({ queryKey: ["groups"] });
    // A new group isn't in any backup yet.
    queryClient.invalidateQueries({ queryKey: ["backup-status"] });
  });
  useTauriEvent<CeremonyEventPayload>("dkg:failed", (p) => onFailed("dkg", p));

  useTauriEvent<CeremonyEventPayload>("signing:progress", (p) =>
    onProgress("signing", p)
  );
  useTauriEvent<CeremonyEventPayload>("signing:complete", (p) => {
    onComplete("signing", p);
    refreshWallet();
  });
  useTauriEvent<CeremonyEventPayload>("signing:failed", (p) =>
    onFailed("signing", p)
  );

  // Wallet send: a coordinator signing ceremony over a transaction sighash.
  // Captured globally so progress lands even when off the wallet screen.
  useTauriEvent<CeremonyEventPayload>("send:progress", (p) => onProgress("send", p));
  useTauriEvent<CeremonyEventPayload>("send:complete", (p) => {
    onComplete("send", p);
    refreshWallet();
    notifyIfAway("Your payment was approved and sent.");
  });
  useTauriEvent<CeremonyEventPayload>("send:failed", (p) => {
    onFailed("send", p);
    notifyIfAway("Your payment couldn't be sent. Open Cyze to see why.");
  });

  return null;
}
