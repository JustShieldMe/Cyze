import { expect, test } from "vitest";
import { fireEvent, waitFor } from "@testing-library/react";
import { fakeBackend, GROUP, GROUP_ID, renderApp } from "./fake";

const NEW_ID = "6e" + "1".repeat(60) + "07";
const NEW_GROUP = { ...GROUP, id: NEW_ID, description: "Treasury 2", threshold: 3, num_participants: 4 };

test("a group moves to a new one: payees and rules carried over, then everything sent there with the usual approvals", async () => {
  const backend = fakeBackend({
    list_groups: () => [GROUP, NEW_GROUP],
    viewing_key_status: () => ({ private: true, old_address: null, progress: null }),
    group_rules: () => ({ rules: { trusted_payees_only: true }, poll_id: "r1", title: "Rules", threshold: 2, members: 3 }),
    copy_payees: () => 2,
    create_poll: () => "p1",
    group_orchard_keys: ({ id }) => ({ address: id === NEW_ID ? "u1newgroup" : "u1group", ufvk: "uview1" }),
    own_group_for_address: ({ address }) => (address === "u1newgroup" ? { group_id: NEW_ID, description: "Treasury 2" } : null),
    wallet_prepare_send_max: ({ recipient }) => ({
      pczt_hex: "00",
      sighash_hex: "9f".repeat(32),
      spends: [{ pool: "ironwood", index: 0, alpha_hex: "00" }],
      fee_zatoshis: 15_000,
      amount_zatoshis: 1_234_485_000,
      recipient,
      memo: null,
      is_unshield: false,
      approval_window: { expiry_height: 3_500_040, blocks_left: 37, closes_at_ms: Date.now() + 46 * 60_000 },
    }),
  });
  const ui = await renderApp(`/groups/${GROUP_ID}`);
  fireEvent.click(await ui.findByRole("button", { name: "Change members or threshold" }));
  fireEvent.change(await ui.findByRole("combobox", { name: "New group" }), { target: { value: NEW_ID } });

  fireEvent.click(await ui.findByRole("button", { name: "Copy payees" }));
  await ui.findByText("Copied 2 payees.");
  expect(backend.calls("copy_payees")).toEqual([{ groupId: GROUP_ID, toGroupId: NEW_ID }]);
  fireEvent.click(ui.getByRole("button", { name: "Propose the same rules" }));
  await ui.findByText(/Asked the new group in/);
  expect(backend.calls("create_poll")[0]).toMatchObject({
    args: { group_id: NEW_ID, title: "Adopt the rules of “Treasury”", rules: { trusted_payees_only: true } },
  });

  const move = ui.getByRole("button", { name: "Move the funds" }) as HTMLButtonElement;
  await waitFor(() => expect(move.disabled).toBe(false));
  fireEvent.click(move);
  await ui.findByText("Review payment");
  expect(backend.calls("set_active_wallet")).toContainEqual({ groupId: GROUP_ID });
  expect(backend.calls("wallet_prepare_send_max")).toEqual([{ groupId: GROUP_ID, recipient: "u1newgroup", memo: null }]);
  await ui.findByText("Sends everything available now. The network fee comes out of it.");
  await waitFor(() =>
    expect(document.querySelector(".payee-name")?.textContent).toBe("Your group “Treasury 2” · its address, checked on this device"),
  );
  ui.getByText(/Moving the funds to the group “Treasury 2”/);
});
