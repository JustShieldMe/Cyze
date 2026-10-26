import { expect, test } from "vitest";
import { fireEvent, waitFor, within } from "@testing-library/react";
import { ALICE, BOB, fakeBackend, GROUP_ID, ME, PAYEE, payee, renderApp, started } from "./fake";

const OTHER = "u1other0example0address00000000000000000000000000000000000000000";

const batchDraft = ({ payments }: Record<string, unknown>) => {
  const list = payments as { recipient: string; amount_zatoshis: number; memo: string | null }[];
  return {
    pczt_hex: "00",
    sighash_hex: "9f".repeat(32),
    spends: [{ pool: "ironwood", index: 0, alpha_hex: "00" }],
    fee_zatoshis: 20_000,
    amount_zatoshis: list.reduce((t, p) => t + p.amount_zatoshis, 0),
    recipient: "",
    memo: null,
    is_unshield: false,
    payments: list.map((p) => ({ ...p, transparent: false })),
    approval_window: null,
  };
};

test("several payments are entered, reviewed, and sent for approval as one", async () => {
  const backend = fakeBackend({
    list_payees: () => [payee("Auditor", PAYEE, BOB)],
    wallet_prepare_payments: batchDraft,
    wallet_send: () => started("batch-1", 325_000_000),
  });
  const ui = await renderApp("/");
  await ui.findByText("12.345");
  fireEvent.click(ui.getByRole("button", { name: "Send" }));
  fireEvent.click(ui.getByRole("button", { name: "Pay several at once" }));

  // Two payments: one to a saved payee, one typed, with a memo.
  fireEvent.change(await ui.findByRole("combobox", { name: "Payment 1: pay a saved payee" }), {
    target: { value: "id-Auditor" },
  });
  fireEvent.change(ui.getByRole("textbox", { name: "Payment 1: amount" }), { target: { value: "2.5" } });
  fireEvent.change(ui.getByRole("textbox", { name: "Payment 2: to" }), { target: { value: OTHER } });
  fireEvent.change(ui.getByRole("textbox", { name: "Payment 2: amount" }), { target: { value: "0.75" } });
  fireEvent.change(ui.getByRole("textbox", { name: "Payment 2: memo" }), { target: { value: "March" } });
  fireEvent.change(ui.getByPlaceholderText(/Only your group sees this/), { target: { value: "Q1 contractors" } });
  expect(ui.getByText(/2 payments · 3.25 ZEC/)).toBeTruthy();
  fireEvent.click(ui.getByRole("button", { name: "Review payments" }));

  await ui.findByText("Review 2 payments");
  expect(backend.calls("wallet_prepare_payments")).toEqual([
    {
      groupId: GROUP_ID,
      payments: [
        { recipient: PAYEE, amount_zatoshis: 250_000_000, memo: null },
        { recipient: OTHER, amount_zatoshis: 75_000_000, memo: "March" },
      ],
    },
  ]);
  expect(document.querySelector(".batch-payments .payee-name")?.textContent).toBe("Auditor");
  expect(ui.getByText("Memo: March")).toBeTruthy();
  expect(ui.getByText("3.2502 ZEC")).toBeTruthy(); // total with the fee
  await ui.findByRole("checkbox", { name: /me \(You\)/, checked: true });
  fireEvent.click(ui.getByRole("checkbox", { name: /Alice/ }));
  fireEvent.click(ui.getByRole("checkbox", { name: /The recipients are correct/ }));
  fireEvent.click(ui.getByRole("button", { name: "Request approvals" }));

  await waitFor(() =>
    expect(backend.calls("wallet_send")[0]).toEqual({
      args: {
        group_id: GROUP_ID,
        recipient: "",
        amount_zatoshis: 325_000_000,
        signers: [ME, ALICE],
        memo: null,
        from_old_address: false,
        send_max: false,
        reason: "Q1 contractors",
        payments: [
          { recipient: PAYEE, amount_zatoshis: 250_000_000, memo: null },
          { recipient: OTHER, amount_zatoshis: 75_000_000, memo: "March" },
        ],
      },
    }),
  );
});

test("a pasted list fills the payments, by payee name or address, and says which lines it couldn't read", async () => {
  fakeBackend({ list_payees: () => [payee("Auditor", PAYEE, BOB)] });
  const ui = await renderApp("/");
  await ui.findByText("12.345");
  fireEvent.click(ui.getByRole("button", { name: "Send" }));
  fireEvent.click(ui.getByRole("button", { name: "Pay several at once" }));
  fireEvent.click(ui.getByRole("button", { name: "Paste a list" }));
  fireEvent.change(ui.getByRole("textbox", { name: "Payments to paste" }), {
    target: { value: `auditor, 2.5, Invoice 12, final\n${OTHER}\t0.75\nnot a payment` },
  });
  fireEvent.click(ui.getByRole("button", { name: "Add these" }));

  expect((ui.getByRole("textbox", { name: "Payment 1: to" }) as HTMLInputElement).value).toBe(PAYEE);
  expect((ui.getByRole("textbox", { name: "Payment 1: memo" }) as HTMLInputElement).value).toBe("Invoice 12, final");
  expect((ui.getByRole("textbox", { name: "Payment 2: amount" }) as HTMLInputElement).value).toBe("0.75");
  expect(ui.queryByRole("textbox", { name: "Payment 3: to" })).toBeNull();
  expect(ui.getByText("Couldn't read this line: not a payment")).toBeTruthy();
});

test("history shows a batch as its payments, named where they're payees", async () => {
  fakeBackend({
    wallet_history: () => [
      {
        txid: "ab".repeat(32),
        block_height: 3_499_990,
        timestamp: 1_760_000_000,
        direction: "send",
        amount_zatoshis: 325_000_000,
        fee_zatoshis: 20_000,
        memo: "March",
        recipient: OTHER,
        payments: [
          { recipient: PAYEE, amount_zatoshis: 250_000_000, memo: null },
          { recipient: OTHER, amount_zatoshis: 75_000_000, memo: "March" },
        ],
      },
    ],
    list_payees: () => [payee("Auditor", PAYEE, BOB)],
  });
  const ui = await renderApp("/");
  const activity = within((await ui.findByRole("heading", { name: "Activity" })).closest(".card") as HTMLElement);
  fireEvent.click(await activity.findByText("2 payments"));
  const payments = within(activity.getByText("Payments").closest("tr") as HTMLElement);
  expect(payments.getByText("Auditor")).toBeTruthy();
  expect(payments.getByText(OTHER)).toBeTruthy();
  expect(payments.getByText("Memo: March")).toBeTruthy();
});
