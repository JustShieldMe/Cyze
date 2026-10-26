import { expect, test } from "vitest";
import { fireEvent, waitFor, within } from "@testing-library/react";
import { ALICE, BOB, fakeBackend, GROUP_ID, ME, PAYEE, payee, renderApp } from "./fake";

const ZEC = 100_000_000;
const rulesInEffect = (rules: object, poll_id: string | null = "r1") => ({
  rules,
  poll_id,
  title: poll_id ? "New payment rules" : null,
  threshold: 2,
  members: 3,
});
const card = async (ui: Awaited<ReturnType<typeof renderApp>>, heading: string) =>
  within((await ui.findByRole("heading", { name: heading })).closest(".card") as HTMLElement);

test("a member trusts a payee another member added; trusted ones say so", async () => {
  const backend = fakeBackend({
    list_payees: () => [
      payee("Auditor", PAYEE, ALICE),
      payee("Rent", "u1rent", BOB, { trusted_by: [BOB, ALICE], trusted: true }),
    ],
    trust_payee: () => null,
  });
  const ui = await renderApp("/");
  const payees = await card(ui, "Payees");
  await payees.findByText("Auditor");
  expect(payees.getByText("1 of 2")).toBeTruthy();
  expect(payees.getByText("trusted")).toBeTruthy();
  fireEvent.click(payees.getAllByRole("button", { name: "Trust" })[0]);
  await waitFor(() =>
    expect(backend.calls("trust_payee")).toEqual([{ groupId: GROUP_ID, owner: ALICE, payeeId: "id-Auditor", trusted: true }]),
  );
});

test("rules are proposed from the wallet, as a poll the group adopts", async () => {
  const backend = fakeBackend({ create_poll: () => "r2" });
  const ui = await renderApp("/");
  const rules = await card(ui, "Rules");
  await rules.findByText("No rules: any payment 2 members approve goes through.");
  fireEvent.click(rules.getByRole("button", { name: "Propose rules" }));
  fireEvent.click(rules.getByRole("checkbox", { name: /Pay only trusted payees/ }));
  fireEvent.click(rules.getByRole("checkbox", { name: /Larger payments need more approvals/ }));
  fireEvent.change(rules.getByRole("textbox", { name: "From this amount" }), { target: { value: "100" } });
  expect(rules.getByText("Proposes: Pay only trusted payees. Payments of 100 ZEC or more need 3 approvals.")).toBeTruthy();
  fireEvent.click(rules.getByRole("button", { name: "Ask the group" }));
  await waitFor(() =>
    expect(backend.calls("create_poll")).toEqual([
      {
        args: {
          group_id: GROUP_ID,
          title: "New payment rules",
          details: "Pay only trusted payees.\nPayments of 100 ZEC or more need 3 approvals.",
          options: [],
          open_for_secs: 3 * 86_400,
          rules: { trusted_payees_only: true, large: { from_zatoshis: 100 * ZEC, approvals: 3 } },
        },
      },
    ]),
  );
  await rules.findByText(/Asked the group/);
});

const adoptedPoll = () => [
      {
        group_id: GROUP_ID,
        poll_id: "r1",
        author: ALICE,
        title: "New payment rules",
        details: "",
        options: ["Adopt", "Don't adopt"],
        created_at: Math.floor(Date.now() / 1000) - 600,
        closes_at: Math.floor(Date.now() / 1000) + 86_400,
        closed: false,
        votes: { [ALICE]: 0, [ME]: 0 },
        tally: [2, 0],
        my_vote: 0,
        members: 3,
        threshold: 2,
        agreed: 0,
        community: null,
        rules: { trusted_payees_only: true },
      },
];

test("rules in effect are listed with the poll that adopted them", async () => {
  fakeBackend({ group_rules: () => rulesInEffect({ trusted_payees_only: true }), list_polls: adoptedPoll });
  const ui = await renderApp("/");
  const rules = await card(ui, "Rules");
  await rules.findByText("Pay only trusted payees.");
  expect(rules.getByText("Adopted in the poll “New payment rules”.")).toBeTruthy();
});

test("under the rules, an untrusted address can't be paid, and a large payment asks more members", async () => {
  fakeBackend({
    group_rules: () =>
      rulesInEffect({ trusted_payees_only: true, large: { from_zatoshis: 2 * ZEC, approvals: 3 } }),
    list_payees: () => [payee("Auditor", PAYEE, BOB, { trusted_by: [BOB, ALICE], trusted: true })],
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
  fireEvent.change(ui.getByPlaceholderText(/u1… address/), { target: { value: "u1stranger" } });
  fireEvent.change(ui.getByPlaceholderText("0.00"), { target: { value: "2.5" } });
  await ui.findByText("The group's rules allow payments only to trusted payees.");
  expect((ui.getByRole("button", { name: "Review payment" }) as HTMLButtonElement).disabled).toBe(true);

  fireEvent.change(ui.getByPlaceholderText(/u1… address/), { target: { value: PAYEE } });
  fireEvent.click(ui.getByRole("button", { name: "Review payment" }));
  await ui.findByText(/3 of 3 needed, by the group's rules for larger payments/);
  await ui.findByRole("checkbox", { name: /me \(You\)/, checked: true });
  fireEvent.click(ui.getByRole("checkbox", { name: /Alice/ }));
  fireEvent.click(ui.getByRole("checkbox", { name: /The recipient is correct/ }));
  const request = ui.getByRole("button", { name: "Request approvals" }) as HTMLButtonElement;
  expect(request.disabled).toBe(true);
  expect(ui.getByText("Choose 1 more. Each must be online to approve.")).toBeTruthy();
  fireEvent.click(ui.getByRole("checkbox", { name: /Bob/ }));
  expect(request.disabled).toBe(false);
});

test("a poll proposing rules says what it proposes, and that they're in effect once adopted", async () => {
  fakeBackend({ group_rules: () => rulesInEffect({ trusted_payees_only: true }), list_polls: adoptedPoll });
  // Last in this file: the app's router keeps the page it navigates to.
  const ui = await renderApp("/");
  fireEvent.click(await ui.findByRole("link", { name: /Votes/ }));
  await ui.findByText("Proposes these rules, replacing the group's");
  expect(ui.getByText("Pay only trusted payees.")).toBeTruthy();
  await ui.findByText("✓ Adopted: these rules are in effect.");
});
