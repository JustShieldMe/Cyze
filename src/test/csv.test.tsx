import { expect, test } from "vitest";
import { cell, coins, historyCsv } from "../lib/csv";
import type { Payee, TxRecord } from "../ipc/commands";

test("cells are quoted when they need to be, and never read as formulas", () => {
  expect(cell("plain")).toBe("plain");
  expect(cell('Invoice "12", final')).toBe('"Invoice ""12"", final"');
  expect(cell("line one\nline two")).toBe('"line one\nline two"');
  expect(cell("=HYPERLINK(\"x\")")).toBe('"\'=HYPERLINK(""x"")"');
  expect(cell("-5")).toBe("-5");
  expect(cell("-2.50000000")).toBe("-2.50000000");
  expect(cell("-rm")).toBe("'-rm");
  expect(cell(null)).toBe("");
});

test("amounts keep all eight decimals", () => {
  expect(coins(250_015_000)).toBe("2.50015000");
  expect(coins(1)).toBe("0.00000001");
});

test("history exports one row per payment, with payees, reasons and who signed", () => {
  const sent: TxRecord = {
    txid: "ab".repeat(32),
    block_height: 3_500_000,
    timestamp: 1_760_000_000,
    direction: "send",
    amount_zatoshis: 325_000_000,
    fee_zatoshis: 20_000,
    memo: null,
    recipient: null,
    payments: [
      { recipient: "u1auditor", amount_zatoshis: 250_000_000, memo: null },
      { recipient: "u1other", amount_zatoshis: 75_000_000, memo: "March" },
    ],
  };
  const received: TxRecord = {
    txid: "cd".repeat(32),
    block_height: null,
    timestamp: null,
    direction: "receive",
    amount_zatoshis: 1_000_000_000,
    fee_zatoshis: null,
    memo: "Q3 funding",
    recipient: null,
  };
  const auditor = { payee_id: "p", name: "Auditor", address: "u1auditor", forms: ["u1auditor"] } as Payee;
  const csv = historyCsv([sent, received], {
    unit: "ZEC",
    payees: [auditor],
    reasons: [{ txid: sent.txid, author: "aa", text: "Q1 contractors", at: 1 }],
    signers: { [sent.txid]: ["aa", "ee"] },
    name: (pk) => ({ aa: "Alice", ee: "me (You)" })[pk] ?? pk,
  });
  expect(csv.split("\r\n")).toEqual([
    "Date (UTC),Block,Type,Amount (ZEC),Fee (ZEC),Payee,Address,Memo,Reason,Signed by,Transaction ID",
    `2025-10-09 08:53:20,3500000,Sent,-2.50000000,0.00020000,Auditor,u1auditor,,Q1 contractors (Alice),"Alice, me (You)",${sent.txid}`,
    `2025-10-09 08:53:20,3500000,Sent,-0.75000000,,,u1other,March,Q1 contractors (Alice),"Alice, me (You)",${sent.txid}`,
    `pending,,Received,10.00000000,,,,Q3 funding,,,${received.txid}`,
    "",
  ]);
});
