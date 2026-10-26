import { expect, test } from "vitest";
import { emit } from "@tauri-apps/api/event";
import { fakeBackend, ALICE, GROUP_ID, ME, PAYEE, renderApp } from "./fake";

const DAY = 86_400;
const now = () => Math.floor(Date.now() / 1000);
let state: { votes: Record<string, number>; announcedAgo: number } = { votes: {}, announcedAgo: 3_600 };

const announcement = () => ({
  group_id: GROUP_ID,
  poll_id: "p9",
  author: ME,
  title: "Pay the auditor's retainer?",
  details: "",
  options: ["Pay", "Don't pay"],
  created_at: now() - state.announcedAgo,
  announced_at: now() - state.announcedAgo,
  closes_at: now() + 7 * DAY,
  closed: false,
  votes: state.votes,
  tally: [0, 1].map((c) => Object.values(state.votes).filter((v) => v === c).length),
  my_vote: null,
  members: 3,
  threshold: 2,
  agreed: null,
  community: null,
  payments: [{ recipient: PAYEE, amount_zatoshis: 250_000_000, memo: null }],
  paid_txid: null,
});

test("an announcement waits out the rules' delay, can be vetoed, and then its author can send it", async () => {
  fakeBackend({
    list_polls: () => [announcement()],
    group_rules: () => ({ rules: { delay: { from_zatoshis: 100_000_000, secs: DAY } }, poll_id: "r1", title: "Wait", threshold: 2, members: 3 }),
  });
  const ui = await renderApp("/votes");
  await ui.findByText(/Announced. The group's rules make it wait 1 day: it can be sent in 23 hours/);
  expect(ui.queryByRole("button", { name: "Send payment" })).toBeNull();

  state = { votes: { [ALICE]: 1 }, announcedAgo: 2 * DAY };
  await emit("polls:updated", { group_id: GROUP_ID, new_polls: 0 });
  await ui.findByText("Vetoed: Alice chose Don't pay. It can't be sent while anyone does.");

  state = { votes: { [ALICE]: 0 }, announcedAgo: 2 * DAY };
  await emit("polls:updated", { group_id: GROUP_ID, new_polls: 0 });
  await ui.findByText("The wait is over with no veto. You can send it now.");
  expect(ui.getByRole("button", { name: "Send payment" })).toBeTruthy();
});
