import { expect, test } from "vitest";
import { fireEvent, waitFor, within } from "@testing-library/react";
import { fakeBackend, GROUP_ID, renderApp } from "./fake";

const ZEC = 100_000_000;
const WEEK = 7 * 86_400;
const limit = { up_to_zatoshis: ZEC, per_secs: WEEK };

test("a member turns on automatic approval within the group's limit, and sees how much is used", async () => {
  let on = false;
  const backend = fakeBackend({
    group_rules: () => ({ rules: { limit }, poll_id: "r1", title: "Small payments", threshold: 2, members: 3 }),
    auto_approval_status: () => ({ on, spent: 40_000_000, limit }),
    set_auto_approve: ({ on: value }) => {
      on = value as boolean;
      return null;
    },
  });
  const ui = await renderApp("/");
  const rules = within((await ui.findByRole("heading", { name: "Rules" })).closest(".card") as HTMLElement);
  await rules.findByText(/Members' apps may approve payments to trusted payees by themselves, up to 1 ZEC in any 7 days/);
  await rules.findByText("The wallet has sent 0.4 ZEC of 1 ZEC in the last 7 days.");
  fireEvent.click(rules.getByRole("checkbox", { name: /Approve automatically on this device/ }));
  await waitFor(() => expect(backend.calls("set_auto_approve")).toEqual([{ groupId: GROUP_ID, on: true }]));
  await waitFor(() => expect((rules.getByRole("checkbox", { name: /Approve automatically/ }) as HTMLInputElement).checked).toBe(true));
});

test("a spending limit is proposed with the rules", async () => {
  const backend = fakeBackend({ create_poll: () => "r4" });
  const ui = await renderApp("/");
  const rules = within((await ui.findByRole("heading", { name: "Rules" })).closest(".card") as HTMLElement);
  fireEvent.click(await rules.findByRole("button", { name: "Propose rules" }));
  fireEvent.click(rules.getByRole("checkbox", { name: /Small payments can be approved automatically/ }));
  fireEvent.change(rules.getByRole("textbox", { name: "Limit" }), { target: { value: "1" } });
  fireEvent.click(rules.getByRole("button", { name: "Ask the group" }));
  await waitFor(() =>
    expect(backend.calls("create_poll")[0]).toMatchObject({ args: { rules: { limit: { up_to_zatoshis: ZEC, per_secs: WEEK } } } }),
  );
});
