import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import {
  walletPreparePayments,
  walletSyncNow,
  AppError,
  DraftTransaction,
  GroupSummary,
  MAX_PAYMENTS,
  Payee,
  PaymentRequest,
  WalletNetwork,
} from "../ipc/commands";
import { unit } from "../network";
import { MAX_REASON, payeeFor, usePayees } from "../lib/payees";
import { payeeRuleError, useGroupRules } from "../lib/rules";
import { ADDRESS_PLACEHOLDER, checkRecipient } from "./address";
import { SyncErrorView, zec } from "./common";

/** One payment being entered: the text in its fields. */
export interface PaymentRow {
  recipient: string;
  amount: string;
  memo: string;
}

export const emptyRow = (): PaymentRow => ({ recipient: "", amount: "", memo: "" });

/** The payments a set of rows describes, what's wrong with each row, and
 *  their total. Empty rows are skipped; `payments` is only complete when no
 *  row has an error. `ruleError` says why the group's rules forbid paying an
 *  address, if they do. */
export function checkRows(
  rows: PaymentRow[],
  network: WalletNetwork,
  ruleError: (recipient: string) => string | null = () => null,
): { payments: PaymentRequest[]; errors: (string | null)[]; total: number } {
  const payments: PaymentRequest[] = [];
  let total = 0;
  const errors = rows.map((row) => {
    const recipient = row.recipient.trim();
    if (!recipient && !row.amount.trim()) return null;
    if (!recipient) return "Who is this payment to?";
    const check = checkRecipient(recipient, network);
    if (check.error) return check.error;
    if (/\.zcash$/i.test(recipient)) return "Use the address here; names are looked up one payment at a time.";
    const forbidden = ruleError(recipient);
    if (forbidden) return forbidden;
    const zats = Math.round(Number(row.amount) * 1e8);
    if (!(zats > 0)) return "Enter an amount.";
    total += zats;
    const memo = check.transparent ? null : row.memo.trim() || null;
    payments.push({ recipient, amount_zatoshis: zats, memo });
    return null;
  });
  return { payments, errors, total };
}

/** Parse pasted lines of "address or payee name, amount, memo" (commas or
 *  tabs; the memo may contain commas). Returns the rows it read and the lines
 *  it couldn't. */
export function parseList(text: string, payees: Payee[] | undefined): { rows: PaymentRow[]; bad: string[] } {
  const rows: PaymentRow[] = [];
  const bad: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const [to, amount, ...rest] = line.split(/\t|,/).map((f) => f.trim());
    const payee = payees?.find((p) => p.name.toLowerCase() === to?.toLowerCase());
    if (!to || !amount || !(Number(amount) > 0)) {
      bad.push(line.trim());
      continue;
    }
    rows.push({ recipient: payee?.address ?? to, amount, memo: rest.join(", ") });
  }
  return { rows, bad };
}

/** Why the group's rules forbid paying an address, if they do. */
export function useRuleError(groupId: string): (recipient: string) => string | null {
  const payees = usePayees(groupId);
  const rules = useGroupRules(groupId);
  return (recipient) => payeeRuleError(rules.data?.rules, payees.data, recipient);
}

/** Rows of payments: to (typed, or one of the group's payees), amount and an
 *  optional memo each. */
export function PaymentRows({
  groupId,
  network,
  rows,
  onChange,
}: {
  groupId: string;
  network: WalletNetwork;
  rows: PaymentRow[];
  onChange: (rows: PaymentRow[]) => void;
}) {
  const payees = usePayees(groupId);
  const ruleError = useRuleError(groupId);
  const { errors } = checkRows(rows, network, ruleError);
  const [pasting, setPasting] = useState(false);
  const [pasted, setPasted] = useState("");
  const [badLines, setBadLines] = useState<string[]>([]);
  const set = (i: number, change: Partial<PaymentRow>) =>
    onChange(rows.map((row, j) => (j === i ? { ...row, ...change } : row)));

  const addPasted = () => {
    const { rows: read, bad } = parseList(pasted, payees.data);
    const kept = rows.filter((r) => r.recipient.trim() || r.amount.trim());
    onChange([...kept, ...read].slice(0, MAX_PAYMENTS));
    setBadLines(bad);
    if (!bad.length) {
      setPasted("");
      setPasting(false);
    }
  };

  return (
    <>
      {rows.map((row, i) => {
        const payee = payeeFor(payees.data, row.recipient);
        const transparent = checkRecipient(row.recipient, network).transparent;
        return (
          <div className="payment-row" key={i}>
            <div className="row between items-center">
              <span className="dim text-sm">Payment {i + 1}</span>
              <div className="row gap-2 items-center">
                {!!payees.data?.length && (
                  <select
                    className="w-auto payee-pick"
                    aria-label={`Payment ${i + 1}: pay a saved payee`}
                    value={payee?.payee_id ?? ""}
                    onChange={(e) => {
                      const picked = payees.data?.find((p) => p.payee_id === e.target.value);
                      if (picked) set(i, { recipient: picked.address });
                    }}
                  >
                    <option value="" disabled>
                      Pay a saved payee…
                    </option>
                    {payees.data.map((p) => (
                      <option key={`${p.author}/${p.payee_id}`} value={p.payee_id}>
                        {p.name}
                      </option>
                    ))}
                  </select>
                )}
                {rows.length > 1 && (
                  <button
                    className="link-button danger-link"
                    onClick={() => onChange(rows.filter((_, j) => j !== i))}
                  >
                    Remove
                  </button>
                )}
              </div>
            </div>
            <input
              type="text"
              aria-label={`Payment ${i + 1}: to`}
              placeholder={ADDRESS_PLACEHOLDER[network]}
              value={row.recipient}
              onChange={(e) => set(i, { recipient: e.target.value })}
            />
            {payee && <p className="ok pull-up text-sm">{payee.name} · a group payee</p>}
            <div className="row gap-2">
              <input
                type="text"
                inputMode="decimal"
                className="payment-amount"
                aria-label={`Payment ${i + 1}: amount`}
                placeholder={`0.00 ${unit(network)}`}
                value={row.amount}
                onChange={(e) => set(i, { amount: e.target.value })}
              />
              {!transparent && (
                <input
                  type="text"
                  aria-label={`Payment ${i + 1}: memo`}
                  placeholder="Memo (optional, only the recipient reads it)"
                  value={row.memo}
                  maxLength={512}
                  onChange={(e) => set(i, { memo: e.target.value })}
                />
              )}
            </div>
            {errors[i] && <div className="error pull-up">{errors[i]}</div>}
          </div>
        );
      })}
      <div className="row gap-2 mb-3">
        {rows.length < MAX_PAYMENTS && (
          <button className="secondary" onClick={() => onChange([...rows, emptyRow()])}>
            Add a payment
          </button>
        )}
        <button className="link-button" onClick={() => setPasting((p) => !p)}>
          {pasting ? "Close the list" : "Paste a list"}
        </button>
      </div>
      {pasting && (
        <div className="callout block mb-3">
          <label className="mt-0">One payment per line: address or payee name, amount, memo</label>
          <textarea
            aria-label="Payments to paste"
            rows={4}
            placeholder={"Auditor, 2.5, Invoice 12\nu1…, 0.75"}
            value={pasted}
            onChange={(e) => setPasted(e.target.value)}
          />
          {badLines.length > 0 && (
            <div className="error">
              Couldn't read {badLines.length === 1 ? "this line" : "these lines"}: {badLines.join(" · ")}
            </div>
          )}
          <button onClick={addPasted} disabled={!pasted.trim()}>
            Add these
          </button>
        </div>
      )}
    </>
  );
}

/** Several payments in one transaction (a batch), approved once: payroll,
 *  grants. */
export function BatchSend({
  group,
  network,
  spendable,
  blocked,
  onPrepared,
  onSingle,
}: {
  group: GroupSummary;
  network: WalletNetwork;
  spendable: number;
  /** Why nothing can be sent right now, if so. */
  blocked: string | null;
  onPrepared: (draft: DraftTransaction, reason?: string) => void;
  onSingle: () => void;
}) {
  const [rows, setRows] = useState<PaymentRow[]>([emptyRow(), emptyRow()]);
  const [reason, setReason] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const ruleError = useRuleError(group.id);
  const { payments, errors, total } = checkRows(rows, network, ruleError);
  const tooMuch = total > spendable;
  const prepare = useMutation({
    mutationFn: () => walletPreparePayments(group.id, payments),
    onSuccess: (d) => onPrepared(d, reason.trim() || undefined),
    onError: (e) => {
      const appErr = e as unknown as AppError;
      setErr(appErr.message);
      if (appErr.code === "not_synced") void walletSyncNow(false);
    },
  });
  const u = unit(network);

  return (
    <div className="card panel">
      <div className="row between items-center">
        <h3 className="m-0">Pay several at once</h3>
        <button className="link-button" onClick={onSingle}>
          One payment
        </button>
      </div>
      <p className="dim text-sm">
        Up to {MAX_PAYMENTS} payments in one transaction, approved once: payroll, grants, invoices.
      </p>
      <PaymentRows groupId={group.id} network={network} rows={rows} onChange={setRows} />

      <label>Reason (optional)</label>
      <textarea
        placeholder="What they're for. Only your group sees this."
        value={reason}
        maxLength={MAX_REASON}
        rows={2}
        onChange={(e) => setReason(e.target.value)}
      />
      <p className="dim text-sm">
        {payments.length} {payments.length === 1 ? "payment" : "payments"} · {zec(total)} {u}
        {tooMuch ? ` · more than the ${zec(spendable)} ${u} available` : ` of ${zec(spendable)} ${u} available`}.
        The network fee is added on top.
      </p>
      {err && <SyncErrorView text={err} />}
      {blocked && <p className="dim text-sm">{blocked}</p>}
      <button
        onClick={() => prepare.mutate()}
        disabled={!!blocked || prepare.isPending || payments.length < 2 || errors.some(Boolean) || tooMuch}
      >
        {prepare.isPending ? "Preparing…" : "Review payments"}
      </button>
    </div>
  );
}
