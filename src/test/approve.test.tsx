import { expect, test, vi } from "vitest";
import { fireEvent, waitFor } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { useCeremonies } from "../stores/ceremonies";
import { ALICE, fakeBackend, GROUP_ID, renderApp } from "./fake";

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
  await ui.findByText("Request from Alice");
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
  await ui.findByText("Request from Alice");
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
