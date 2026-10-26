import { expect, test } from "vitest";
import { waitFor } from "@testing-library/react";
import { ALICE, fakeBackend, GROUP_ID, ME, PAYEE, renderApp } from "./fake";

// Its own file: the scheduler starts once the history loads on unlock.
test("a scheduled payment that can't be asked for yet says why and isn't asked again at once", async () => {
  const due = Math.floor(Date.now() / 1000) - 60;
  const rent = {
    group_id: GROUP_ID,
    schedule_id: "s1",
    author: ME,
    payments: [
      { recipient: PAYEE, amount_zatoshis: 100_000_000, memo: null },
      { recipient: "u1other", amount_zatoshis: 50_000_000, memo: null },
    ],
    reason: "",
    every: { days: 7 },
    first_due: due,
    signers: [ME, ALICE],
    stopped: false,
    runs: {},
    due_now: due,
    next_due: due + 7 * 86_400,
    missed: [],
    mine: true,
  };
  const backend = fakeBackend({
    due_schedules: () => [rent],
    list_schedules: () => [rent],
    wallet_send: () => Promise.reject({ code: "not_synced", message: "The wallet is still catching up" }),
  });
  const ui = await renderApp("/");
  await ui.findByText(/Couldn't ask for the payment due .*: The wallet is still catching up. Trying again shortly./);
  const sends = backend.calls("wallet_send");
  expect(sends).toHaveLength(1);
  expect(sends[0].args).toMatchObject({
    reason: "Scheduled payment",
    payments: [
      { recipient: PAYEE, amount_zatoshis: 100_000_000, memo: null },
      { recipient: "u1other", amount_zatoshis: 50_000_000, memo: null },
    ],
  });
  await waitFor(() => expect(backend.calls("wallet_sync_now").length).toBeGreaterThan(0));
  expect(backend.calls("wallet_send")).toHaveLength(1);
});
