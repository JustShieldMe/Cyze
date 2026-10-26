import type { PaymentReason, Payee, TxRecord } from "../ipc/commands";
import { payeeFor, reasonsFor } from "./payees";

/** One cell: quoted when it has to be, and with a leading quote mark when a
 *  spreadsheet would otherwise run it as a formula (a memo starting with "=").
 *  A plain number, such as a negative amount, stays a number. */
export function cell(value: string | number | null | undefined): string {
  let text = value === null || value === undefined ? "" : String(value);
  if (/^[=+\-@\t\r]/.test(text) && !/^-?\d+(\.\d+)?$/.test(text)) text = `'${text}`;
  return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

/** An amount in zatoshis as coins with all eight decimals: exact, no rounding. */
export function coins(zatoshis: number): string {
  const whole = Math.floor(zatoshis / 100_000_000);
  return `${whole}.${String(zatoshis % 100_000_000).padStart(8, "0")}`;
}

/** A wallet's history as CSV, one row per payment (a batch has one per
 *  payment, its fee on the first), with payee names, reasons and, where this
 *  device knows them, who signed. */
export function historyCsv(
  history: TxRecord[],
  {
    unit,
    payees,
    reasons,
    signers,
    name,
  }: {
    unit: string;
    payees?: Payee[];
    reasons?: PaymentReason[];
    signers: Record<string, string[] | undefined>;
    name: (pubkey: string) => string;
  },
): string {
  const header = [
    "Date (UTC)",
    "Block",
    "Type",
    `Amount (${unit})`,
    `Fee (${unit})`,
    "Payee",
    "Address",
    "Memo",
    "Reason",
    "Signed by",
    "Transaction ID",
  ];
  const lines = [header.map(cell).join(",")];
  for (const tx of history) {
    const date = tx.timestamp !== null ? new Date(tx.timestamp * 1000).toISOString().replace("T", " ").slice(0, 19) : "pending";
    const type = { receive: "Received", send: "Sent", move: "Moved to the private address" }[tx.direction] ?? tx.direction;
    const why = reasonsFor(reasons, tx.txid)
      .map((r) => `${r.text} (${name(r.author)})`)
      .join(" | ");
    const signedBy = (signers[tx.txid] ?? []).map(name).join(", ");
    const payments =
      tx.direction === "send" && tx.payments && tx.payments.length > 1
        ? tx.payments
        : [{ recipient: tx.direction === "send" ? tx.recipient : null, amount_zatoshis: tx.amount_zatoshis, memo: tx.memo }];
    payments.forEach((p, i) => {
      const sign = tx.direction === "receive" ? "" : tx.direction === "send" ? "-" : "";
      const fee = i === 0 && tx.fee_zatoshis !== null ? coins(tx.fee_zatoshis) : "";
      lines.push(
        [
          date,
          tx.block_height ?? "",
          type,
          `${sign}${coins(p.amount_zatoshis)}`,
          fee,
          payeeFor(payees, p.recipient)?.name ?? "",
          p.recipient ?? "",
          p.memo ?? "",
          why,
          signedBy,
          tx.txid,
        ]
          .map(cell)
          .join(","),
      );
    });
  }
  return lines.join("\r\n") + "\r\n";
}
