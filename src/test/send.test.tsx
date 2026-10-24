import { expect, test } from "vitest";
import { fireEvent, waitFor } from "@testing-library/react";
import { ALICE, fakeBackend, GROUP_ID, ME, renderApp } from "./fake";

const PAYEE = "u1payee0example0address0000000000000000000000000000000000000000";

test("a payment is entered, reviewed, and sent for approval", async () => {
  const backend = fakeBackend({
    wallet_prepare_send: ({ recipient, amountZatoshis, memo }) => ({
      pczt_hex: "00",
      sighash_hex: "9f".repeat(32),
      spends: [{ pool: "ironwood", index: 0, alpha_hex: "00" }],
      fee_zatoshis: 15_000,
      amount_zatoshis: amountZatoshis,
      recipient,
      memo,
      is_unshield: false,
    }),
    wallet_send: () => "send-1",
  });
  const ui = await renderApp("/");

  // Step 1: who, how much, and a memo.
  await ui.findByText("12.345");
  fireEvent.click(ui.getByRole("button", { name: "Send" }));
  fireEvent.change(ui.getByPlaceholderText(/u1… address/), { target: { value: PAYEE } });
  fireEvent.change(ui.getByPlaceholderText("0.00"), { target: { value: "2.5" } });
  fireEvent.change(ui.getByPlaceholderText(/Only the recipient can read it/), { target: { value: "March invoice" } });
  fireEvent.click(ui.getByRole("button", { name: "Review payment" }));

  // Step 2: the prepared transaction, who approves, and the mainnet check.
  const request = (await ui.findByRole("button", { name: "Request approvals" })) as HTMLButtonElement;
  expect(backend.calls("wallet_prepare_send")).toEqual([
    { groupId: GROUP_ID, recipient: PAYEE, amountZatoshis: 250_000_000, memo: "March invoice" },
  ]);
  expect(ui.getByText("2.50015 ZEC")).toBeTruthy(); // amount plus fee
  expect(request.disabled).toBe(true); // one more approver, and the recipient check
  fireEvent.click(ui.getByRole("checkbox", { name: /Alice/ }));
  expect(request.disabled).toBe(true);
  fireEvent.click(ui.getByRole("checkbox", { name: /The recipient is correct/ }));
  expect(request.disabled).toBe(false);
  fireEvent.click(request);

  await waitFor(() =>
    expect(backend.calls("wallet_send")).toEqual([
      {
        args: {
          group_id: GROUP_ID,
          recipient: PAYEE,
          amount_zatoshis: 250_000_000,
          signers: [ME, ALICE],
          memo: "March invoice",
          from_old_address: false,
        },
      },
    ]),
  );
});

test("an address for another network is caught before anything is built", async () => {
  const backend = fakeBackend();
  const ui = await renderApp("/");
  await ui.findByText("12.345");
  fireEvent.click(ui.getByRole("button", { name: "Send" }));
  fireEvent.change(ui.getByPlaceholderText(/u1… address/), { target: { value: "utest1somebodyontestnet" } });
  fireEvent.change(ui.getByPlaceholderText("0.00"), { target: { value: "1" } });
  expect(ui.getByText(/This address is for a different network/)).toBeTruthy();
  expect((ui.getByRole("button", { name: "Review payment" }) as HTMLButtonElement).disabled).toBe(true);
  expect(backend.calls("wallet_prepare_send")).toEqual([]);
});
