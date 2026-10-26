import { expect, test } from "vitest";
import { emit } from "@tauri-apps/api/event";
import { fireEvent, waitFor } from "@testing-library/react";
import { ALICE, BOB, fakeBackend, GROUP_ID, ME, PAYEE, payee, renderApp, started } from "./fake";

const paymentPoll = (author: string, extra: Record<string, unknown> = {}) => ({
  group_id: GROUP_ID,
  poll_id: "p1",
  author,
  title: "Pay the auditor for Q4?",
  details: "",
  options: ["Pay", "Don't pay"],
  created_at: Math.floor(Date.now() / 1000) - 60,
  closes_at: Math.floor(Date.now() / 1000) + 3 * 86_400,
  closed: false,
  votes: { [ME]: 0, [ALICE]: 0 },
  tally: [2, 0],
  my_vote: 0,
  members: 3,
  threshold: 2,
  agreed: 0,
  community: null,
  payments: [{ recipient: PAYEE, amount_zatoshis: 250_000_000, memo: "Q4 audit" }],
  paid_txid: null,
  ...extra,
});

test("a member proposes a payment in a poll, which members answer Pay or Don't pay", async () => {
  const backend = fakeBackend({ list_payees: () => [payee("Auditor", PAYEE, BOB)], create_poll: () => "p1" });
  const ui = await renderApp("/votes");
  await ui.findByText(/No open polls/);
  fireEvent.click(ui.getByRole("button", { name: "New poll" }));
  fireEvent.click(await ui.findByRole("checkbox", { name: "Propose a payment" }));
  expect(ui.queryByText("Options")).toBeNull();
  fireEvent.change(ui.getByPlaceholderText(/Pay the auditor for Q4/), { target: { value: "Pay the auditor for Q4?" } });
  fireEvent.change(await ui.findByRole("combobox", { name: "Payment 1: pay a saved payee" }), { target: { value: "id-Auditor" } });
  fireEvent.change(ui.getByRole("textbox", { name: "Payment 1: amount" }), { target: { value: "2.5" } });
  fireEvent.change(ui.getByRole("textbox", { name: "Payment 1: memo" }), { target: { value: "Q4 audit" } });
  fireEvent.click(ui.getByRole("button", { name: "Ask the group" }));
  await waitFor(() =>
    expect(backend.calls("create_poll")).toEqual([
      {
        args: {
          group_id: GROUP_ID,
          title: "Pay the auditor for Q4?",
          details: "",
          options: [],
          open_for_secs: 3 * 86_400,
          payments: [{ recipient: PAYEE, amount_zatoshis: 250_000_000, memo: "Q4 audit" }],
        },
      },
    ]),
  );
});

test("once the group agrees, the member who proposed it sends exactly it, asking those who chose Pay", async () => {
  const backend = fakeBackend({
    list_polls: () => [paymentPoll(ME)],
    list_payees: () => [payee("Auditor", PAYEE, BOB)],
    wallet_send: () => started("send-p1"),
  });
  const ui = await renderApp("/votes");
  await ui.findByText("Proposes paying");
  await ui.findByText("Auditor");
  expect(ui.getByText("The group agreed. You can send it now.")).toBeTruthy();
  fireEvent.click(ui.getByRole("button", { name: "Send payment" }));
  await ui.findByText(/Payment started/);
  expect(backend.calls("wallet_send")).toEqual([
    {
      args: {
        group_id: GROUP_ID,
        recipient: PAYEE,
        amount_zatoshis: 250_000_000,
        signers: [ME, ALICE],
        memo: "Q4 audit",
        from_old_address: false,
        send_max: false,
        reason: "Pay the auditor for Q4?",
        poll_id: "p1",
      },
    },
  ]);
});

test("other members see who can send an agreed payment, and a paid poll says so", async () => {
  let paid = false;
  fakeBackend({ list_polls: () => [paymentPoll(ALICE, paid ? { paid_txid: "ab".repeat(32) } : {})] });
  const ui = await renderApp("/votes");
  await ui.findByText("Agreed to pay. Alice can send it now.");
  expect(ui.queryByRole("button", { name: "Send payment" })).toBeNull();
  paid = true;
  await emit("polls:updated", { group_id: GROUP_ID, new_polls: 0 });
  await ui.findByText(/✓ Paid, in transaction/);
});
