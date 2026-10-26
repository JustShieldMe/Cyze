import { expect, test } from "vitest";
import { fireEvent, waitFor, within } from "@testing-library/react";
import { BOB, fakeBackend, GROUP_ID, PAYEE, payee, renderApp } from "./fake";

const ZEC = 100_000_000;
const DAY = 86_400;

test("a delay with veto is proposed with the rules", async () => {
  const backend = fakeBackend({ create_poll: () => "r3" });
  const ui = await renderApp("/");
  const rules = within((await ui.findByRole("heading", { name: "Rules" })).closest(".card") as HTMLElement);
  fireEvent.click(await rules.findByRole("button", { name: "Propose rules" }));
  fireEvent.click(rules.getByRole("checkbox", { name: /Larger payments wait for vetoes/ }));
  fireEvent.change(rules.getByRole("textbox", { name: "Wait from this amount" }), { target: { value: "50" } });
  expect(
    rules.getByText("Proposes: Payments of 50 ZEC or more are announced 2 days ahead, and any member can veto them."),
  ).toBeTruthy();
  fireEvent.click(rules.getByRole("button", { name: "Ask the group" }));
  await waitFor(() =>
    expect(backend.calls("create_poll")[0]).toMatchObject({ args: { rules: { delay: { from_zatoshis: 50 * ZEC, secs: 2 * DAY } } } }),
  );
});

test("a payment the rules make wait is announced instead of sent", async () => {
  const backend = fakeBackend({
    group_rules: () => ({
      rules: { delay: { from_zatoshis: 2 * ZEC, secs: DAY } },
      poll_id: "r1",
      title: "Wait for vetoes",
      threshold: 2,
      members: 3,
    }),
    list_payees: () => [payee("Auditor", PAYEE, BOB, { trusted: true })],
    create_poll: () => "p9",
  });
  const ui = await renderApp("/");
  await ui.findByText("12.345");
  fireEvent.click(ui.getByRole("button", { name: "Send" }));
  fireEvent.change(ui.getByPlaceholderText(/u1… address/), { target: { value: PAYEE } });
  fireEvent.change(ui.getByPlaceholderText("0.00"), { target: { value: "2.5" } });
  fireEvent.change(ui.getByPlaceholderText(/Only your group sees this/), { target: { value: "Annual retainer" } });
  await ui.findByText(/make payments of 2 ZEC or more wait 1 day for vetoes/);
  expect(ui.queryByRole("button", { name: "Review payment" })).toBeNull();
  fireEvent.click(ui.getByRole("button", { name: "Announce it" }));
  await waitFor(() =>
    expect(backend.calls("create_poll")).toEqual([
      {
        args: {
          group_id: GROUP_ID,
          title: "Pay 2.5 ZEC?",
          details: "Annual retainer",
          options: [],
          open_for_secs: DAY + 7 * DAY,
          payments: [{ recipient: PAYEE, amount_zatoshis: 250_000_000, memo: null }],
        },
      },
    ]),
  );
  await ui.findByText(/Announced. You can send it from/);
});
