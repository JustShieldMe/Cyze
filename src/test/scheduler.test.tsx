import { expect, test } from "vitest";
import { emit } from "@tauri-apps/api/event";
import { waitFor } from "@testing-library/react";
import { ALICE, fakeBackend, GROUP_ID, ME, PAYEE, renderApp, started } from "./fake";

// Its own file: the scheduler starts once the history loads on unlock.
test("a due scheduled payment is asked of its approvers once, and recorded sent when it goes through", async () => {
  const due = Math.floor(Date.now() / 1000) - 60;
  const rent = {
    group_id: GROUP_ID,
    schedule_id: "s1",
    author: ME,
    payments: [{ recipient: PAYEE, amount_zatoshis: 150_000_000, memo: "March" }],
    reason: "Office rent",
    every: { months: 1 },
    first_due: due,
    signers: [ME, ALICE],
    stopped: false,
    runs: {},
    due_now: due,
    next_due: due + 30 * 86_400,
    missed: [],
    mine: true,
  };
  const backend = fakeBackend({
    due_schedules: () => [rent],
    list_schedules: () => [rent],
    wallet_send: () => started("send-s1", 150_000_000),
  });
  const ui = await renderApp("/");
  await waitFor(() =>
    expect(backend.calls("wallet_send")).toEqual([
      {
        args: {
          group_id: GROUP_ID,
          recipient: PAYEE,
          amount_zatoshis: 150_000_000,
          signers: [ME, ALICE],
          memo: "March",
          from_old_address: false,
          send_max: false,
          reason: "Scheduled: Office rent",
        },
      },
    ]),
  );
  await ui.findByText(/is waiting for approvals/);

  const txid = "ab".repeat(32);
  await emit("send:complete", { ceremony_id: "send-s1", group_id: GROUP_ID, txid });
  await waitFor(() =>
    expect(backend.calls("record_schedule_run")[0]).toEqual({ groupId: GROUP_ID, scheduleId: "s1", due, txid }),
  );
  expect(backend.calls("wallet_send")).toHaveLength(1);
});
