import { expect, test } from "vitest";
import { fireEvent, waitFor } from "@testing-library/react";
import { ALICE, BOB, fakeBackend, GROUP_ID, ME, renderApp, started } from "./fake";

test("an older group's members are offered a private viewing key", async () => {
  let started = false;
  const backend = fakeBackend({
    viewing_key_status: () =>
      started
        ? {
            private: false,
            old_address: null,
            progress: { session_id: "v1", started_by: null, state: "waiting", waiting_for: [BOB], error: null },
          }
        : { private: false, old_address: null, progress: null },
    make_viewing_key_private: () => {
      started = true;
      return null;
    },
  });
  const ui = await renderApp("/");
  await ui.findByText(/Anyone who knows this group's ID can see its balance/);
  fireEvent.click(ui.getByRole("button", { name: "Make private" }));
  fireEvent.click(ui.getByRole("button", { name: "Start" }));
  await ui.findByText(/Waiting for Bob to open Cyze/);
  expect(backend.calls("make_viewing_key_private")).toEqual([{ groupId: GROUP_ID }]);
});

test("funds left at the old address are moved with the group's approval", async () => {
  const backend = fakeBackend({
    viewing_key_status: () => ({
      private: true,
      old_address: { total_zatoshis: 1_000_000_000, spendable_zatoshis: 1_000_000_000, synced: true },
      progress: null,
    }),
    wallet_prepare_move: () => ({
      pczt_hex: "00",
      sighash_hex: "9f".repeat(32),
      spends: [{ pool: "ironwood", index: 0, alpha_hex: "00" }],
      fee_zatoshis: 10_000,
      amount_zatoshis: 999_990_000,
      recipient: "u1private",
      memo: null,
      is_unshield: false,
    }),
    wallet_send: () => started("send-2"),
  });
  const ui = await renderApp("/");
  await ui.findByText(/is still at the old address/);
  fireEvent.click(ui.getByRole("button", { name: "Move funds" }));
  await ui.findByText("Review: move to the private address");
  expect(ui.getByText("This wallet's private address")).toBeTruthy();
  await ui.findByRole("checkbox", { name: /me \(You\)/, checked: true });
  fireEvent.click(ui.getByRole("checkbox", { name: /Alice/ }));
  fireEvent.click(ui.getByRole("button", { name: "Request approvals" }));
  await waitFor(() =>
    expect(backend.calls("wallet_send")).toEqual([
      {
        args: {
          group_id: GROUP_ID,
          recipient: "u1private",
          amount_zatoshis: 999_990_000,
          signers: [ME, ALICE],
          memo: null,
          from_old_address: true,
          send_max: false,
          reason: null,
        },
      },
    ]),
  );
});
