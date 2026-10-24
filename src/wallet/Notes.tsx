import { useQuery } from "@tanstack/react-query";
import {
  walletNotes,
  WalletNetwork,
} from "../ipc/commands";
import { unit } from "../network";

import { zec } from "./common";

/** "Review Notes" tab: lists the unspent shielded notes (Orchard + Ironwood)
 *  that make up the balance. Because each note is one spend authorization (one FROST round per
 *  signer), this also surfaces the round-cost of a full-balance send and offers
 *  a one-click self-send consolidation to merge fragmented notes into one. */
export function ReviewNotesTab({
  groupId,
  network,
  onConsolidate,
  consolidatePending,
  disabledReason,
}: {
  groupId: string;
  network: WalletNetwork;
  onConsolidate: () => void;
  consolidatePending: boolean;
  disabledReason: string | null;
}) {
  const notes = useQuery({
    queryKey: ["wallet-notes", groupId],
    queryFn: () => walletNotes(groupId),
    refetchInterval: 15_000,
  });
  const list = notes.data ?? [];
  const spendable = list.filter((n) => n.status === "spendable");
  const pending = list.filter((n) => n.status !== "spendable");
  const totalSpendable = spendable.reduce((a, n) => a + n.value_zatoshis, 0);
  const rounds = spendable.length;
  const canConsolidate = spendable.length >= 2 && !disabledReason;

  const badge = (s: string) =>
    s === "spendable"
      ? { cls: "green", label: "spendable" }
      : s === "spending"
        ? { cls: "blue", label: "spending" }
        : { cls: "", label: "pending" };

  return (
    <div className="card" style={{ marginTop: 14, background: "var(--bg-elevated)" }}>
      <h3 style={{ marginTop: 0 }}>Notes in this wallet</h3>
      {notes.isLoading ? (
        <p className="dim">Loading notes…</p>
      ) : list.length === 0 ? (
        <p className="dim">
          No notes yet — this wallet hasn't received any funds. Notes appear here
          once a deposit confirms.
        </p>
      ) : (
        <>
          <p className="dim" style={{ marginTop: 0 }}>
            Your balance is made of <strong>{spendable.length}</strong> spendable
            note{spendable.length === 1 ? "" : "s"}
            {pending.length > 0 && <> (plus {pending.length} pending)</>}, totalling{" "}
            <strong>{zec(totalSpendable)} {unit(network)}</strong> spendable. Each note is a
            separate spend authorization, so a full-balance send needs{" "}
            <strong>
              {rounds} signing round{rounds === 1 ? "" : "s"}
            </strong>{" "}
            — one approval per note for every signer.
          </p>

          {spendable.length >= 2 && (
            <div className="callout" style={{ marginBottom: 12 }}>
              <span>
                <strong>Consolidate</strong> merges these {spendable.length} notes
                into a single note with one self-send to the group's own address.
                Future transactions then need only one signing round. It costs a
                small network fee and one signing ceremony now.
              </span>
              <div style={{ marginTop: 8 }}>
                <button
                  onClick={onConsolidate}
                  disabled={!canConsolidate || consolidatePending}
                >
                  {consolidatePending
                    ? "Preparing…"
                    : `Consolidate ${spendable.length} notes → 1`}
                </button>
                {disabledReason && (
                  <span className="dim" style={{ marginLeft: 10, fontSize: 12 }}>
                    {disabledReason}
                  </span>
                )}
              </div>
            </div>
          )}

          <table className="participants" style={{ width: "100%" }}>
            <thead>
              <tr>
                <th style={{ textAlign: "left", paddingBottom: 6 }}>Value</th>
                <th style={{ textAlign: "left", paddingBottom: 6 }}>Status</th>
                <th style={{ textAlign: "right", paddingBottom: 6 }}>Confirmations</th>
                <th style={{ textAlign: "left", paddingBottom: 6, paddingLeft: 12 }}>
                  From tx
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((n, i) => {
                const b = badge(n.status);
                return (
                  <tr key={n.received_txid + i}>
                    <td style={{ fontWeight: 600, whiteSpace: "nowrap" }}>
                      {zec(n.value_zatoshis)} {unit(network)}
                    </td>
                    <td>
                      <span className={`badge ${b.cls}`}>{b.label}</span>
                      {n.is_change && (
                        <span className="dim" style={{ fontSize: 11, marginLeft: 6 }}>
                          change
                        </span>
                      )}
                    </td>
                    <td style={{ textAlign: "right" }}>
                      {n.status === "pending" ? "—" : n.confirmations}
                    </td>
                    <td className="dim mono" style={{ fontSize: 11, paddingLeft: 12 }}>
                      {n.received_txid.slice(0, 12)}…
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </>
      )}
    </div>
  );
}
