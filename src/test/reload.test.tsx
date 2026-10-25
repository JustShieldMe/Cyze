import { expect, test } from "vitest";
import { waitFor } from "@testing-library/react";
import { fakeBackend, renderApp, savedSend } from "./fake";

// Its own file, so the keystore unlocks (and the history loads) afresh.
test("a send still running when the window reloads stays in progress", async () => {
  fakeBackend({ load_ui_state: () => savedSend("live-1"), live_ceremonies: () => ["live-1"] });
  const ui = await renderApp("/");
  await ui.findByText("Payment in progress");
  expect(ui.getByText(/Approvals are needed by about/)).toBeTruthy();
  await waitFor(() => expect(ui.queryByText("Payment failed")).toBeNull());
});
