import { useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createPoll, AppError, PaymentRequest, WalletNetwork } from "../ipc/commands";
import { unit } from "../network";
import { durationText } from "../lib/rules";
import { zec } from "./common";

/** Instead of sending a payment the group's rules make wait for vetoes:
 *  announce it in a payment poll. Once the wait has passed with nobody
 *  choosing Don't pay, its author sends it from Votes. */
export function AnnounceInstead({
  groupId,
  network,
  payments,
  reason,
  delay,
  disabled,
}: {
  groupId: string;
  network: WalletNetwork;
  payments: PaymentRequest[];
  reason?: string;
  delay: { from_zatoshis: number; secs: number };
  disabled: boolean;
}) {
  const queryClient = useQueryClient();
  const total = payments.reduce((t, p) => t + p.amount_zatoshis, 0);
  const u = unit(network);
  const [title, setTitle] = useState(
    payments.length === 1 ? `Pay ${zec(total)} ${u}?` : `Make ${payments.length} payments, ${zec(total)} ${u} in all?`,
  );
  const announce = useMutation({
    mutationFn: () =>
      createPoll({
        group_id: groupId,
        title: title.trim(),
        details: reason ?? "",
        options: [],
        // Open well past the wait, so members can still veto when it's sendable.
        open_for_secs: delay.secs + 7 * 86_400,
        payments,
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["polls"] }),
  });

  if (announce.isSuccess) {
    return (
      <p className="ok mb-0">
        Announced. You can send it from <Link to="/votes">Votes</Link> in {durationText(delay.secs)}, unless a member
        vetoes it.
      </p>
    );
  }
  return (
    <div className="callout block">
      <p className="mt-0">
        The group's rules make payments of {zec(delay.from_zatoshis)} {u} or more wait {durationText(delay.secs)} for
        vetoes. Announce it now: once the wait has passed with no member choosing Don't pay, you can send it.
      </p>
      <label className="mt-0">Question</label>
      <input type="text" value={title} maxLength={200} onChange={(e) => setTitle(e.target.value)} />
      {announce.error && <div className="error">{(announce.error as unknown as AppError).message}</div>}
      <button onClick={() => announce.mutate()} disabled={disabled || announce.isPending || !title.trim()}>
        {announce.isPending ? "Announcing…" : "Announce it"}
      </button>
    </div>
  );
}
