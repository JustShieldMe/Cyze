import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import {
  cancelViewingKeySession,
  getIdentity,
  listContacts,
  makeViewingKeyPrivate,
  viewingKeyStatus,
  AppError,
  GroupSummary,
  WalletNetwork,
} from "../ipc/commands";
import { unit } from "../network";
import { resolveParticipant } from "../lib/participants";
import { zec } from "./common";

/** "Alice", "Alice and Bob", "Alice, Bob and Carol". */
function names(list: string[]): string {
  return list.length <= 1 ? (list[0] ?? "") : `${list.slice(0, -1).join(", ")} and ${list[list.length - 1]}`;
}

/** For a group created before viewing keys were private: make its key private
 *  with the other members, follow that, then move the funds off the old
 *  address. Shows nothing for a group whose key is private and settled. */
export function ViewingKeyCallout({
  group,
  network,
  sendInProgress,
  movePending,
  onMove,
}: {
  group: GroupSummary;
  network: WalletNetwork;
  sendInProgress: boolean;
  movePending: boolean;
  onMove: () => void;
}) {
  const status = useQuery({
    queryKey: ["viewing-key", group.id],
    queryFn: () => viewingKeyStatus(group.id),
    // Refreshed on `viewing_key:updated` and `wallet:updated`; a slow fallback.
    refetchInterval: 60_000,
  });
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const label = (pubkey: string) => resolveParticipant(pubkey, identity.data, contacts.data).label;

  const [confirming, setConfirming] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const start = useMutation({
    mutationFn: () => makeViewingKeyPrivate(group.id),
    onSuccess: () => {
      setErr(null);
      setConfirming(false);
      void status.refetch();
    },
    onError: (e) => setErr((e as unknown as AppError).message ?? String(e)),
  });
  const stop = useMutation({
    mutationFn: () => cancelViewingKeySession(group.id),
    onSuccess: () => void status.refetch(),
  });

  const v = status.data;
  if (!v) return null;
  const p = v.progress;

  if (p && p.state !== "failed") {
    const waiting = p.waiting_for.map(label);
    return (
      <div className="callout" style={{ marginTop: 12, alignItems: "center" }}>
        <span style={{ flex: 1 }}>
          <strong>Making this wallet private</strong>
          {p.started_by ? `, as ${label(p.started_by)} asked. ` : ". "}
          {p.state === "connecting"
            ? "Connecting…"
            : waiting.length
              ? `Waiting for ${names(waiting)} to open Cyze.`
              : "Checking everyone got the same key…"}
        </span>
        <button className="secondary" onClick={() => stop.mutate()} disabled={stop.isPending}>
          Cancel
        </button>
      </div>
    );
  }

  if (p?.state === "failed") {
    return (
      <div className="callout warn" style={{ marginTop: 12, alignItems: "center" }}>
        <span style={{ flex: 1 }}>Couldn't make this wallet private: {p.error}</span>
        <div className="row">
          <button onClick={() => start.mutate()} disabled={start.isPending}>
            Try again
          </button>
          <button className="secondary" onClick={() => stop.mutate()}>
            Dismiss
          </button>
        </div>
      </div>
    );
  }

  if (!v.private) {
    return (
      <div className="callout" style={{ marginTop: 12, display: "block" }}>
        <div className="row" style={{ alignItems: "center" }}>
          <span style={{ flex: 1 }}>Anyone who knows this group's ID can see its balance and payments.</span>
          {!confirming && (
            <button onClick={() => setConfirming(true)}>Make private</button>
          )}
        </div>
        {confirming && (
          <>
            <p className="dim" style={{ marginBottom: 8 }}>
              Every member needs Cyze open while it runs; their apps join by themselves. The wallet then gets a new
              address, and you move its funds there with one payment the group approves.
            </p>
            <div className="row">
              <button onClick={() => start.mutate()} disabled={start.isPending}>
                {start.isPending ? "Starting…" : "Start"}
              </button>
              <button className="secondary" onClick={() => setConfirming(false)}>
                Not now
              </button>
            </div>
          </>
        )}
        {err && <div className="error" style={{ marginBottom: 0 }}>{err}</div>}
      </div>
    );
  }

  const old = v.old_address;
  if (!old || old.total_zatoshis === 0 || sendInProgress) return null;
  const u = unit(network);
  const waitingOnly = old.spendable_zatoshis === 0;
  return (
    <div className="callout" style={{ marginTop: 12, alignItems: "center" }}>
      <span style={{ flex: 1 }}>
        {zec(old.total_zatoshis)} {u} is still at the old address, which anyone with the group's ID can see.
        {waitingOnly && " It can move once it has 10 confirmations."}
        {!waitingOnly && !old.synced && " Checking the old address is up to date…"}
      </span>
      <button onClick={onMove} disabled={movePending || waitingOnly || !old.synced}>
        {movePending ? "Preparing…" : "Move funds"}
      </button>
    </div>
  );
}
