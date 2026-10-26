import { expect, test } from "vitest";
import { fireEvent, waitFor, within } from "@testing-library/react";
import { ALICE, BOB, fakeBackend, GROUP_ID, ME, PAYEE, payee, renderApp } from "./fake";
import { middayOn } from "../wallet/Schedules";

const DAY = 86_400;
const now = () => Math.floor(Date.now() / 1000);
const dateIn = (days: number) => {
  const t = new Date(Date.now() + days * DAY * 1000);
  return `${t.getFullYear()}-${String(t.getMonth() + 1).padStart(2, "0")}-${String(t.getDate()).padStart(2, "0")}`;
};
const day = (secs: number) => new Date(secs * 1000).toLocaleDateString(undefined, { dateStyle: "medium" });

const schedule = (author: string, extra: Record<string, unknown> = {}) => ({
  group_id: GROUP_ID,
  schedule_id: `s-${author.slice(0, 2)}`,
  author,
  payments: [{ recipient: PAYEE, amount_zatoshis: 150_000_000, memo: null }],
  reason: "Office rent",
  every: { months: 1 },
  first_due: now() - 40 * DAY,
  signers: [author, ALICE],
  stopped: false,
  runs: {},
  due_now: null,
  next_due: now() + 5 * DAY,
  missed: [],
  mine: author === ME,
  ...extra,
});

test("a member schedules a monthly payment to a payee, approved by the members they pick", async () => {
  const backend = fakeBackend({ list_payees: () => [payee("Landlord", PAYEE, BOB)], create_schedule: () => "s1" });
  const ui = await renderApp("/");
  await ui.findByText("Nothing scheduled.");
  fireEvent.click(ui.getByRole("button", { name: "Schedule a payment" }));
  fireEvent.change(await ui.findByRole("combobox", { name: "Payment 1: pay a saved payee" }), {
    target: { value: "id-Landlord" },
  });
  fireEvent.change(ui.getByRole("textbox", { name: "Payment 1: amount" }), { target: { value: "1.5" } });
  fireEvent.change(ui.getByPlaceholderText(/March salaries/), { target: { value: " Office rent " } });
  expect((ui.getByLabelText("Repeats") as HTMLSelectElement).selectedOptions[0].textContent).toBe("Every month");
  fireEvent.change(ui.getByLabelText("First payment"), { target: { value: dateIn(5) } });
  await ui.findByText(/around midday on its date/);
  const schedule = ui.getByRole("button", { name: "Schedule" }) as HTMLButtonElement;
  expect(schedule.disabled).toBe(true);
  ui.getByText("Approvers (2 of 3 needed)");
  fireEvent.click(ui.getByRole("checkbox", { name: "Alice" }));
  expect(schedule.disabled).toBe(false);
  fireEvent.click(schedule);
  await waitFor(() =>
    expect(backend.calls("create_schedule")).toEqual([
      {
        args: {
          group_id: GROUP_ID,
          payments: [{ recipient: PAYEE, amount_zatoshis: 150_000_000, memo: null }],
          reason: "Office rent",
          every: { months: 1 },
          first_due: middayOn(dateIn(5)),
          signers: [ME, ALICE],
        },
      },
    ]),
  );
});

test("the group's schedules show when they're next, who approves and what was skipped; only its author stops one", async () => {
  const sentOn = now() - 10 * DAY;
  const missed = now() - 40 * DAY;
  const backend = fakeBackend({
    list_payees: () => [payee("Landlord", PAYEE, BOB)],
    list_schedules: () => [
      schedule(ME, { runs: { [sentOn]: "ab".repeat(32) }, missed: [missed] }),
      schedule(BOB, { reason: "", every: { days: 14 }, signers: [BOB, ME], due_now: now() - 60 }),
    ],
  });
  const ui = await renderApp("/");
  const rent = (await ui.findByText("Office rent")).closest(".schedule") as HTMLElement;
  within(rent).getByText(`Every month · 1.5 ZEC · next on ${day(now() + 5 * DAY)}`);
  within(rent).getByText("Approvers: me (You), Alice");
  within(rent).getByText(`Last sent on ${day(sentOn)}.`);
  within(rent).getByText(/Skipped the payment due .*, when your app wasn't open/);

  const bobs = ui.getByText("To Landlord").closest(".schedule") as HTMLElement;
  within(bobs).getByText(/Every 2 weeks · 1.5 ZEC · next on .* · scheduled by Bob/);
  within(bobs).getByText(/Due .*: Bob's app asks for approvals/);
  expect(within(bobs).queryByRole("button", { name: "Stop" })).toBeNull();

  fireEvent.click(within(rent).getByRole("button", { name: "Stop" }));
  fireEvent.click(within(rent).getByRole("button", { name: "Stop schedule" }));
  await waitFor(() => expect(backend.calls("stop_schedule")).toEqual([{ groupId: GROUP_ID, scheduleId: "s-ee" }]));
});

test("a payment the group's rules would make wait for vetoes can't be scheduled", async () => {
  fakeBackend({
    list_payees: () => [payee("Landlord", PAYEE, BOB)],
    group_rules: () => ({
      rules: { delay: { from_zatoshis: 100_000_000, secs: DAY } },
      poll_id: "r1",
      title: "Rules",
      threshold: 2,
      members: 3,
    }),
  });
  const ui = await renderApp("/");
  fireEvent.click(await ui.findByRole("button", { name: "Schedule a payment" }));
  fireEvent.change(await ui.findByRole("combobox", { name: "Payment 1: pay a saved payee" }), {
    target: { value: "id-Landlord" },
  });
  fireEvent.change(ui.getByRole("textbox", { name: "Payment 1: amount" }), { target: { value: "2" } });
  fireEvent.click(ui.getByRole("checkbox", { name: "Alice" }));
  await ui.findByText(/wait 1 day for vetoes, which a scheduled payment can't/);
  expect((ui.getByRole("button", { name: "Schedule" }) as HTMLButtonElement).disabled).toBe(true);
});
