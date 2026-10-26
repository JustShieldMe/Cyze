import { expect, test, vi } from "vitest";
import { fireEvent, waitFor } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { useCeremonies } from "../stores/ceremonies";
import { ALICE, BOB, fakeBackend, GROUP_ID, payee, renderApp } from "./fake";

const joinedAs = (session: string) => useCeremonies.getState().joinedSessions[session];

const request = { session_id: "s1", kind: "signing", coordinator: "Alice", coordinator_pubkey: ALICE, matching_groups: [GROUP_ID] };
const context = {
  recipient: "u1payee",
  amount_zatoshis: 250_000_000,
  fee_zatoshis: 15_000,
  memo: "March invoice",
  is_unshield: false,
  network: "main",
  plan_id: "p1",
  spend_index: 1,
  spend_total: 1,
  tx_sighash: "9f".repeat(32),
};

test("a signing request is joined by itself, verified, and approved", async () => {
  vi.spyOn(document, "hasFocus").mockReturnValue(false);
  const backend = fakeBackend({
    list_pending_sessions: () => [request],
    join_signing_session: () => "c1",
    respond_to_signing: () => null,
  });
  const ui = await renderApp("/inbox");

  // The app joins it in the background, and says so while it's elsewhere.
  await ui.findByText("Alice asks to approve a payment from Treasury");
  await waitFor(() =>
    expect(backend.calls("join_signing_session")).toEqual([
      { args: { group_id: GROUP_ID, session_id: "s1", server_url: null } },
    ]),
  );
  expect(backend.calls("notify")).toEqual([{ title: "Cyze", body: "A payment is waiting for your approval." }]);

  // The backend checked the transaction itself; the user sees what it found.
  await emit("signing:progress", {
    ceremony_id: "c1",
    event: {
      phase: "awaiting_approval",
      message_hex: "9f".repeat(32),
      context,
      auto_approved: false,
      verified: {
        sighash_hex: "9f".repeat(32),
        expiry_height: 3_500_040,
        payments: [{ address: "u1payee", value_zatoshis: 250_000_000, transparent: false, memo: "March invoice" }],
        spent_zatoshis: 1_000_000_000,
        change_zatoshis: 749_985_000,
        fee_zatoshis: 15_000,
      },
    },
  });
  await ui.findByText(/Verified: this is exactly what your signature would authorize/);
  expect(ui.getByText("“March invoice”")).toBeTruthy();
  expect(ui.getByText("2.5 ZEC")).toBeTruthy();
  fireEvent.click(ui.getByRole("button", { name: "Approve and sign" }));
  await waitFor(() => expect(backend.calls("respond_to_signing")).toEqual([{ ceremonyId: "c1", approve: true }]));
});

test("a request the backend refused says why and can't be approved", async () => {
  fakeBackend({ list_pending_sessions: () => [request], join_signing_session: () => "c2" });
  const ui = await renderApp("/inbox");
  await ui.findByText("Alice asks to approve a payment from Treasury");
  await waitFor(() => expect(joinedAs("s1")).toBe("c2"));
  await emit("signing:progress", {
    ceremony_id: "c2",
    event: {
      phase: "awaiting_approval",
      message_hex: "9f".repeat(32),
      context,
      refused: "refused to sign: the coordinator says it pays 2.5 to u1payee, but it pays 12 to u1thief",
    },
  });
  await ui.findByText("Refused automatically. Nothing was signed.");
  expect(ui.getByText(/^The coordinator says it pays 2.5 to u1payee, but it pays 12 to u1thief$/)).toBeTruthy();
  expect(ui.queryByRole("button", { name: "Approve and sign" })).toBeNull();
});

const verified = {
  sighash_hex: "9f".repeat(32),
  expiry_height: 3_500_040,
  payments: [{ address: "u1payee", value_zatoshis: 250_000_000, transparent: false, memo: null }],
  spent_zatoshis: 1_000_000_000,
  change_zatoshis: 749_985_000,
  fee_zatoshis: 15_000,
};

test("a request shows when it must be approved by, and once expired can only be dismissed", async () => {
  const backend = fakeBackend({
    list_pending_sessions: () => [request, { ...request, session_id: "s2" }],
    join_signing_session: ({ args }) => ((args as { session_id: string }).session_id === "s1" ? "c3" : "c4"),
    respond_to_signing: () => null,
  });
  const ui = await renderApp("/inbox");
  await waitFor(() => expect(joinedAs("s2")).toBe("c4"));
  const event = (ceremony_id: string, window: object) =>
    emit("signing:progress", {
      ceremony_id,
      event: { phase: "awaiting_approval", message_hex: "9f".repeat(32), context, auto_approved: false, verified, window },
    });

  // Still open: approve by the time the approver's own sync server implies.
  await event("c3", { expiry_height: 3_500_040, blocks_left: 20, closes_at_ms: Date.now() + 25 * 60_000 });
  await ui.findByText("Approve by");
  expect(ui.getAllByRole("button", { name: "Approve and sign" })).toHaveLength(1);

  // Past its expiry: nothing to sign, only to dismiss.
  await event("c4", { expiry_height: 3_500_040, blocks_left: 0, closes_at_ms: Date.now() - 60_000 });
  await ui.findByText("This request expired before it was approved. Nothing was signed.");
  expect(ui.getAllByRole("button", { name: "Approve and sign" })).toHaveLength(1);
  fireEvent.click(ui.getByRole("button", { name: "Dismiss" }));
  await waitFor(() => expect(backend.calls("respond_to_signing")).toEqual([{ ceremonyId: "c4", approve: false }]));
});

test("an approver sees the payee's name from their own payees, and what the requester says it's for", async () => {
  fakeBackend({
    list_pending_sessions: () => [request],
    join_signing_session: () => "c5",
    // Matched by the address the transaction verifiably pays, in the form a
    // verified payment shows it.
    list_payees: () => [payee("Auditor", "u1payee-with-more-receivers", BOB, { forms: ["u1payee-with-more-receivers", "u1payee"] })],
  });
  const ui = await renderApp("/inbox");
  await waitFor(() => expect(joinedAs("s1")).toBe("c5"));
  await emit("signing:progress", {
    ceremony_id: "c5",
    event: {
      phase: "awaiting_approval",
      message_hex: "9f".repeat(32),
      context: { ...context, reason: "Audit, invoice 12" },
      auto_approved: false,
      verified,
    },
  });
  await ui.findByText(/Verified: this is exactly what your signature would authorize/);
  expect(ui.getByText("Alice says it's for:").parentElement?.textContent).toBe("Alice says it's for: Audit, invoice 12");
  await waitFor(() =>
    expect(document.querySelector(".payee-name")?.textContent).toBe("Auditor · group payee, added by Bob"),
  );
});

test("an approver sees that the group agreed to the payment a request sends", async () => {
  fakeBackend({
    list_pending_sessions: () => [
      { session_id: "s1", kind: "signing", coordinator: "Alice", coordinator_pubkey: ALICE, matching_groups: [GROUP_ID] },
    ],
    join_signing_session: () => "c6",
  });
  const ui = await renderApp("/inbox");
  await waitFor(() => expect(joinedAs("s1")).toBe("c6"));
  await emit("signing:progress", {
    ceremony_id: "c6",
    event: {
      phase: "awaiting_approval",
      message_hex: "9f".repeat(32),
      context: { network: "main", spend_index: 1, spend_total: 1, reason: "Pay the auditor for Q4?", poll_id: "p1" },
      auto_approved: false,
      verified: {
        sighash_hex: "9f".repeat(32),
        expiry_height: 3_500_040,
        payments: [{ address: "u1payee", value_zatoshis: 250_000_000, transparent: false, memo: "Q4 audit" }],
        spent_zatoshis: 1_000_000_000,
        change_zatoshis: 749_985_000,
        fee_zatoshis: 15_000,
      },
      poll: { poll_id: "p1", missing: false, title: "Pay the auditor for Q4?", agreed: true, declined: false, chose_pay: 2, threshold: 2 },
    },
  });
  await ui.findByText(/✓ Agreed in the poll “Pay the auditor for Q4\?”: 2 chose Pay, 2 needed/);
});

test("an approver sees which of the group's rules a request meets", async () => {
  fakeBackend({ list_pending_sessions: () => [request], join_signing_session: () => "c7" });
  const ui = await renderApp("/inbox");
  await waitFor(() => expect(joinedAs("s1")).toBe("c7"));
  await emit("signing:progress", {
    ceremony_id: "c7",
    event: {
      phase: "awaiting_approval",
      message_hex: "9f".repeat(32),
      context,
      auto_approved: false,
      verified,
      rules: { trusted_payees_only: true, needed: 3, asked: 3 },
    },
  });
  await ui.findByText("✓ Meets the group's rules: pays only trusted payees · 3 approvals needed, 3 asked.");
});
