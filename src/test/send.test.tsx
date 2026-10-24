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
  await ui.findByRole("checkbox", { name: /me \(You\)/, checked: true });
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
          send_max: false,
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

test("Send all pays everything, with the fee coming out of it", async () => {
  const backend = fakeBackend({
    wallet_prepare_send_max: ({ recipient, memo }) => ({
      pczt_hex: "00",
      sighash_hex: "9f".repeat(32),
      spends: [{ pool: "ironwood", index: 0, alpha_hex: "00" }],
      fee_zatoshis: 15_000,
      amount_zatoshis: 1_234_485_000,
      recipient,
      memo,
      is_unshield: false,
    }),
    wallet_send: () => "send-3",
  });
  const ui = await renderApp("/");
  await ui.findByText("12.345");
  fireEvent.click(ui.getByRole("button", { name: "Send" }));
  fireEvent.change(ui.getByPlaceholderText(/u1… address/), { target: { value: PAYEE } });
  fireEvent.click(ui.getByRole("button", { name: "Send all" }));
  expect((ui.getByPlaceholderText("0.00") as HTMLInputElement).value).toBe("12.345");
  fireEvent.click(ui.getByRole("button", { name: "Review payment" }));

  await ui.findByText(/Sends everything available now/);
  expect(backend.calls("wallet_prepare_send_max")).toEqual([{ groupId: GROUP_ID, recipient: PAYEE, memo: null }]);
  expect(backend.calls("wallet_prepare_send")).toEqual([]);
  expect(ui.getByText("12.345 ZEC")).toBeTruthy(); // the total: amount plus fee
  await ui.findByRole("checkbox", { name: /me \(You\)/, checked: true });
  fireEvent.click(ui.getByRole("checkbox", { name: /Alice/ }));
  fireEvent.click(ui.getByRole("checkbox", { name: /The recipient is correct/ }));
  fireEvent.click(ui.getByRole("button", { name: "Request approvals" }));
  await waitFor(() => expect(backend.calls("wallet_send")[0]).toMatchObject({ args: { send_max: true, recipient: PAYEE } }));
});

test("an approver ticked before your identity loads stays ticked", async () => {
  // Your identity arrives late; you tick Alice first.
  fakeBackend({
    get_identity: () =>
      new Promise((resolve) =>
        setTimeout(() => resolve({ username: "me", pubkey: ME, safety_code: "0" }), 300),
      ),
    wallet_prepare_send: ({ recipient, amountZatoshis }) => ({
      pczt_hex: "00",
      sighash_hex: "9f".repeat(32),
      spends: [{ pool: "ironwood", index: 0, alpha_hex: "00" }],
      fee_zatoshis: 15_000,
      amount_zatoshis: amountZatoshis,
      recipient,
      memo: null,
      is_unshield: false,
    }),
  });
  const ui = await renderApp("/");
  await ui.findByText("12.345");
  fireEvent.click(ui.getByRole("button", { name: "Send" }));
  fireEvent.change(ui.getByPlaceholderText(/u1… address/), { target: { value: PAYEE } });
  fireEvent.change(ui.getByPlaceholderText("0.00"), { target: { value: "1" } });
  fireEvent.click(ui.getByRole("button", { name: "Review payment" }));
  await ui.findByRole("button", { name: "Request approvals" });
  fireEvent.click(ui.getByRole("checkbox", { name: /Alice/ }));
  await ui.findByRole("checkbox", { name: /me \(You\)/, checked: true });
  expect((ui.getByRole("checkbox", { name: /Alice/ }) as HTMLInputElement).checked).toBe(true);
});
