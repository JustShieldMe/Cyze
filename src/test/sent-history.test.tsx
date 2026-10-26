// Its own file: the ceremony store reads saved sends once per module, so this
// can't follow tests that already loaded it.
import { expect, test } from "vitest";
import { fireEvent, within } from "@testing-library/react";
import { BOB, fakeBackend, GROUP_ID, PAYEE, payee, renderApp } from "./fake";

const OTHER = "u1other0example0address00000000000000000000000000000000000000000";

test("history shows a send this device made as it was sent, though the wallet recorded less", async () => {
  const txid = "cd".repeat(32);
  fakeBackend({
    // What the wallet knows of its own send: the balance change, fee included.
    wallet_history: () => [
      { txid, block_height: 3_499_990, timestamp: 1_760_000_000, direction: "send", amount_zatoshis: 325_020_000, fee_zatoshis: null, memo: null, recipient: null, payments: [] },
    ],
    load_ui_state: () =>
      JSON.stringify({
        version: 0,
        state: {
          activeDkgId: null,
          activeSigningId: null,
          txSigners: {},
          activeSendByGroup: {},
          ceremonies: {
            "batch-9": {
              kind: "send",
              phase: "complete",
              done: true,
              failed: false,
              groupId: GROUP_ID,
              txid,
              startedAt: Date.now() - 3 * 60 * 60_000,
              send: {
                groupId: GROUP_ID,
                recipient: "",
                amountZatoshis: 325_000_000,
                feeZatoshis: 20_000,
                sighashHex: "9f".repeat(32),
                payments: [
                  { recipient: PAYEE, amountZatoshis: 250_000_000 },
                  { recipient: OTHER, amountZatoshis: 75_000_000, memo: "March" },
                ],
              },
            },
          },
        },
      }),
    list_payees: () => [payee("Auditor", PAYEE, BOB)],
  });
  const ui = await renderApp("/");
  const activity = within((await ui.findByRole("heading", { name: "Activity" })).closest(".card") as HTMLElement);
  // The saved send arrives once the store has read it back.
  fireEvent.click(await activity.findByText("2 payments", undefined, { timeout: 3000 }));
  expect(activity.getAllByText("−3.25 ZEC").length).toBeGreaterThan(0);
  expect(activity.getByText("0.0002 ZEC")).toBeTruthy(); // the fee, as built
  expect(within(activity.getByText("Payments").closest("tr") as HTMLElement).getByText("Auditor")).toBeTruthy();
});
