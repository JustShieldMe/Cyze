import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getIdentity, removePayee, savePayee, trustPayee, AppError, GroupSummary, Payee, WalletNetwork } from "../ipc/commands";
import { MAX_PAYEE_NAME, useMemberName, useOwnGroup, usePayees } from "../lib/payees";
import { ADDRESS_PLACEHOLDER, checkRecipient } from "./address";

/** Says a payment goes to one of this member's own groups, when it does. */
export function OwnGroupName({ address }: { address: string | null | undefined }) {
  const own = useOwnGroup(address);
  if (!own) return null;
  return (
    <div className="payee-name">
      Your group “{own.description}” <span className="dim">· its address, checked on this device</span>
    </div>
  );
}

/** An address shortened for a table cell; the full one is in its title. */
export function shortAddress(address: string): string {
  return address.length > 24 ? `${address.slice(0, 12)}…${address.slice(-6)}` : address;
}

/** The group's shared payees: addresses it pays, with a name every member
 *  sees. Anyone can add one; only the member who added it can change it. */
export function GroupPayees({ group, network }: { group: GroupSummary; network: WalletNetwork }) {
  const payees = usePayees(group.id);
  const memberName = useMemberName();
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState<Payee | "new" | null>(null);
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const me = identity.data?.pubkey ?? null;
  const remove = useMutation({
    mutationFn: (p: Payee) => removePayee(group.id, p.payee_id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["payees", group.id] }),
  });
  const trust = useMutation({
    mutationFn: ({ p, trusted }: { p: Payee; trusted: boolean }) => trustPayee(group.id, p.author, p.payee_id, trusted),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["payees", group.id] }),
  });
  if (!group.ciphersuite.includes("Pallas")) return null;
  const list = payees.data ?? [];

  return (
    <div className="card mt-4">
      <div className="row between items-center">
        <h3 className="m-0">Payees</h3>
        {editing === null && (
          <button className="secondary" onClick={() => setEditing("new")}>
            Add a payee
          </button>
        )}
      </div>
      <p className="dim text-sm">
        Addresses the group pays, with a name every member sees. When a payment goes to one, approvers see its
        name. Only the member who added a payee can change or remove it. A payee is trusted once{" "}
        {group.threshold} members trust its address.

      </p>
      {editing && (
        <PayeeForm
          groupId={group.id}
          network={network}
          payee={editing === "new" ? null : editing}
          onDone={() => setEditing(null)}
        />
      )}
      {list.length > 0 ? (
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Address</th>
              <th>Added by</th>
              <th>Trust</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {list.map((p) => (
              <tr key={`${p.author}/${p.payee_id}`}>
                <td>{p.name}</td>
                <td className="mono-cell" title={p.address}>
                  {shortAddress(p.address)}
                </td>
                <td className="dim">{memberName(p.author)}</td>
                <td title={p.trusted_by.map(memberName).join(", ")}>
                  {p.trusted ? (
                    <span className="badge green">trusted</span>
                  ) : (
                    <span className="dim text-sm">
                      {p.trusted_by.length} of {group.threshold}
                    </span>
                  )}
                  {!p.mine && me && (
                    <button
                      className="link-button ml-2"
                      disabled={trust.isPending}
                      onClick={() => trust.mutate({ p, trusted: !p.trusted_by.includes(me) })}
                    >
                      {p.trusted_by.includes(me) ? "Stop trusting" : "Trust"}
                    </button>
                  )}
                </td>
                <td className="text-right">
                  {p.mine && (
                    <>
                      <button className="link-button" onClick={() => setEditing(p)}>
                        Edit
                      </button>{" "}
                      <button
                        className="link-button danger-link"
                        onClick={() => {
                          if (confirm(`Remove ${p.name} from the group's payees?`)) remove.mutate(p);
                        }}
                      >
                        Remove
                      </button>
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        !editing && <p className="dim m-0">No payees yet.</p>
      )}
      {(remove.error || trust.error) && (
        <div className="error">{((remove.error ?? trust.error) as unknown as AppError).message}</div>
      )}
    </div>
  );
}

/** Add a payee, or change one this member added. With `address`, saves that
 *  address (from the send form) and only asks for a name. */
export function PayeeForm({
  groupId,
  network,
  payee = null,
  address: fixedAddress,
  onDone,
}: {
  groupId: string;
  network: WalletNetwork;
  payee?: Payee | null;
  address?: string;
  onDone: (saved: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState(payee?.name ?? "");
  const [typed, setTyped] = useState(payee?.address ?? "");
  const address = fixedAddress ?? typed;
  const check = checkRecipient(address, network);
  const save = useMutation({
    mutationFn: () => savePayee(groupId, payee?.payee_id ?? null, name, address),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["payees", groupId] });
      onDone(true);
    },
  });

  return (
    <div className="callout block mb-3">
      <label className="mt-0">Payee name</label>
      <input
        type="text"
        placeholder="e.g. Auditor"
        value={name}
        maxLength={MAX_PAYEE_NAME}
        onChange={(e) => setName(e.target.value)}
      />
      {fixedAddress === undefined && (
        <>
          <label>Address</label>
          <input
            type="text"
            placeholder={ADDRESS_PLACEHOLDER[network]}
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
          />
          {address.trim() && check.error && <div className="error pull-up">{check.error}</div>}
        </>
      )}
      <p className="dim text-sm">Everyone in the group sees this name, and that you added it.</p>
      {save.error && <div className="error">{(save.error as unknown as AppError).message}</div>}
      <div className="row">
        <button
          onClick={() => save.mutate()}
          disabled={save.isPending || !name.trim() || !address.trim() || !!check.error}
        >
          {save.isPending ? "Saving…" : "Save payee"}
        </button>
        <button className="secondary" onClick={() => onDone(false)} disabled={save.isPending}>
          Cancel
        </button>
      </div>
    </div>
  );
}
