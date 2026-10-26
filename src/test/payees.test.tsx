import { expect, test, vi } from "vitest";
import { fireEvent, waitFor, within } from "@testing-library/react";
import { ALICE, fakeBackend, GROUP_ID, ME, PAYEE, payee, renderApp } from "./fake";

test("the group's payees show who added each, and only your own can be changed", async () => {
  vi.spyOn(window, "confirm").mockReturnValue(true);
  const backend = fakeBackend({
    list_payees: () => [payee("Auditor", PAYEE, ALICE), payee("Rent", "u1rent", ME)],
    save_payee: () => "new-id",
    remove_payee: () => null,
  });
  const ui = await renderApp("/");
  const card = within((await ui.findByRole("heading", { name: "Payees" })).closest(".card") as HTMLElement);
  await card.findByText("Auditor");
  expect(card.getByText("Alice")).toBeTruthy();
  expect(card.getByText("me (You)")).toBeTruthy();
  expect(card.getAllByRole("button", { name: "Edit" })).toHaveLength(1);
  expect(card.getAllByRole("button", { name: "Remove" })).toHaveLength(1);

  fireEvent.click(card.getByRole("button", { name: "Add a payee" }));
  fireEvent.change(card.getByPlaceholderText("e.g. Auditor"), { target: { value: "Payroll" } });
  fireEvent.change(card.getByPlaceholderText(/u1… address/), { target: { value: "u1payroll" } });
  fireEvent.click(card.getByRole("button", { name: "Save payee" }));
  await waitFor(() =>
    expect(backend.calls("save_payee")).toEqual([{ groupId: GROUP_ID, payeeId: null, name: "Payroll", address: "u1payroll" }]),
  );

  fireEvent.click(card.getByRole("button", { name: "Remove" }));
  await waitFor(() => expect(backend.calls("remove_payee")).toEqual([{ groupId: GROUP_ID, payeeId: "id-Rent" }]));
});

test("history names payees and shows what payments were for, and you can say why", async () => {
  const sent = "ab".repeat(32);
  const received = "cd".repeat(32);
  const backend = fakeBackend({
    wallet_history: () => [
      { txid: sent, block_height: 3_499_990, timestamp: 1_760_000_000, direction: "send", amount_zatoshis: 100_000_000, fee_zatoshis: 15_000, memo: null, recipient: PAYEE },
      { txid: received, block_height: 3_499_900, timestamp: 1_759_000_000, direction: "receive", amount_zatoshis: 500_000_000, fee_zatoshis: null, memo: null, recipient: null },
    ],
    list_payees: () => [payee("Auditor", PAYEE, ALICE)],
    list_payment_reasons: () => [{ txid: sent, author: ALICE, text: "Audit, invoice 12", at: 1_760_000_100 }],
    set_payment_reason: () => null,
  });
  const ui = await renderApp("/");
  const activity = within((await ui.findByRole("heading", { name: "Activity" })).closest(".card") as HTMLElement);
  await activity.findByText("Auditor");
  expect(activity.getByText("Audit, invoice 12")).toBeTruthy();

  // Expanded, the payment says who said what; you can add yours to another.
  fireEvent.click(activity.getByText("↓ Received"));
  fireEvent.click(await activity.findByRole("button", { name: "Say what it was for" }));
  fireEvent.change(activity.getByRole("textbox", { name: "Reason" }), { target: { value: "Grant, first tranche" } });
  fireEvent.click(activity.getByRole("button", { name: "Save" }));
  await waitFor(() =>
    expect(backend.calls("set_payment_reason")).toEqual([{ groupId: GROUP_ID, txid: received, text: "Grant, first tranche" }]),
  );
});
