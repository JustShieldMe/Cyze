import { expect, test } from "vitest";
import { fireEvent } from "@testing-library/react";
import { ALICE, fakeBackend, GROUP_ID, ME, PAYEE, renderApp, savedSend, started } from "./fake";

test("a send that a lock or restart ended says so, and can be asked again", async () => {
  const backend = fakeBackend({
    load_ui_state: () => savedSend("ended-1"),
    live_ceremonies: () => [],
    wallet_send: () => started("new-1", 100_000_000),
  });
  const ui = await renderApp("/");
  await ui.findByText("Payment failed");
  expect(ui.getByText(/locked or closed before enough members approved. Nothing was sent/)).toBeTruthy();
  fireEvent.click(ui.getByRole("button", { name: "Ask again" }));
  await ui.findByText("Payment in progress");
  expect(backend.calls("wallet_send")).toEqual([
    {
      args: {
        group_id: GROUP_ID,
        recipient: PAYEE,
        amount_zatoshis: 100_000_000,
        signers: [ME, ALICE],
        memo: null,
        from_old_address: false,
        send_max: false,
      },
    },
  ]);
});
