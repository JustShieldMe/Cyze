import { useQuery } from "@tanstack/react-query";
import {
  getIdentity,
  listContacts,
  listPayees,
  listPaymentReasons,
  type Payee,
  type PaymentReason,
} from "../ipc/commands";
import { resolveParticipant } from "./participants";

export const MAX_PAYEE_NAME = 64;
export const MAX_REASON = 500;

/** A group's payees. Kept fresh by `polls:updated` (CeremonyListener), which
 *  fires when any group record arrives. */
export function usePayees(groupId: string | undefined) {
  return useQuery({
    queryKey: ["payees", groupId],
    queryFn: () => listPayees(groupId!),
    enabled: !!groupId,
  });
}

/** What a group's members say its transactions were for. */
export function usePaymentReasons(groupId: string | undefined) {
  return useQuery({
    queryKey: ["payment-reasons", groupId],
    queryFn: () => listPaymentReasons(groupId!),
    enabled: !!groupId,
  });
}

/** The payee a payment to `address` goes to, if it's one of the group's. */
export function payeeFor(payees: Payee[] | undefined, address: string | null | undefined): Payee | undefined {
  const a = address?.trim();
  return a ? payees?.find((p) => p.forms.includes(a)) : undefined;
}

/** What members say transaction `txid` was for. */
export function reasonsFor(reasons: PaymentReason[] | undefined, txid: string | null | undefined): PaymentReason[] {
  const t = txid?.toLowerCase();
  return t ? (reasons ?? []).filter((r) => r.txid === t) : [];
}

/** A member's name, by communication key: "You", a contact, or a short key. */
export function useMemberName(): (pubkey: string) => string {
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  return (pubkey) => resolveParticipant(pubkey, identity.data, contacts.data).label;
}
