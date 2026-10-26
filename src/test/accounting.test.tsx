import { expect, test } from "vitest";
import { fireEvent, within } from "@testing-library/react";
import { fakeBackend, renderApp } from "./fake";

test("the wallet's history is exported as CSV for accounting", async () => {
  const backend = fakeBackend({
    wallet_history: () => [
      { txid: "cd".repeat(32), block_height: 3_499_900, timestamp: 1_760_000_000, direction: "receive", amount_zatoshis: 500_000_000, fee_zatoshis: null, memo: "Q3 funding", recipient: null },
    ],
    save_csv: () => "/home/me/Treasury history.csv",
  });
  const ui = await renderApp("/");
  const activity = within((await ui.findByRole("heading", { name: "Activity" })).closest(".card") as HTMLElement);
  fireEvent.click(await activity.findByRole("button", { name: "Export CSV" }));
  await activity.findByText("Saved to /home/me/Treasury history.csv");
  const [{ fileName, contents }] = backend.calls("save_csv") as { fileName: string; contents: string }[];
  expect(fileName).toMatch(/^Treasury history \d{4}-\d{2}-\d{2}\.csv$/);
  expect(contents.split("\r\n")[0]).toBe(
    "Date (UTC),Block,Type,Amount (ZEC),Fee (ZEC),Payee,Address,Memo,Reason,Signed by,Transaction ID",
  );
  expect(contents).toContain(`2025-10-09 08:53:20,3499900,Received,5.00000000,,,,Q3 funding,,,${"cd".repeat(32)}`);
});
