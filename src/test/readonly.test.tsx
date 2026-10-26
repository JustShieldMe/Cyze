import { expect, test } from "vitest";
import { fireEvent, waitFor } from "@testing-library/react";
import { fakeBackend, GROUP_ID, renderApp } from "./fake";

test("read-only access shows the viewing key only once the member understands what it reveals", async () => {
  fakeBackend({ group_orchard_keys: () => ({ address: "u1group", ufvk: "uview1readonly" }) });
  const ui = await renderApp(`/groups/${GROUP_ID}`);
  fireEvent.click(await ui.findByRole("button", { name: "Share read-only access" }));
  await ui.findByText(/It can't be taken back/);
  expect(ui.queryByText("uview1readonly")).toBeNull();
  fireEvent.click(ui.getByRole("checkbox", { name: /whoever has it sees all of this wallet's payments/ }));
  await waitFor(() => expect(ui.getAllByText("uview1readonly").length).toBeGreaterThan(0));
});
