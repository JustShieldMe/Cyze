import { expect, test } from "vitest";
import { fireEvent, waitFor } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { ALICE, fakeBackend, GROUP_ID, ME, renderApp } from "./fake";

test("a member starts repairing another's lost share, after comparing the new key's safety code", async () => {
  const backend = fakeBackend({
    read_contact: () => ({ name: "bob", pubkey: "dd".repeat(32), safety_code: "11111 22222 33333 44444 55555 66666" }),
    start_repair: () => ({ ceremony_id: "r1", code: "cyzerepair1abc" }),
  });
  const ui = await renderApp(`/groups/${GROUP_ID}`);
  fireEvent.click(await ui.findByRole("button", { name: "Recovery & share repair" }));
  fireEvent.change(ui.getByRole("combobox"), { target: { value: "03" } });
  fireEvent.change(ui.getByPlaceholderText(/zffrost1… from their new device/), { target: { value: "zffrost1bobsnewdevice0000000" } });
  await ui.findByText("11111 22222 33333 44444 55555 66666");
  const start = ui.getByRole("button", { name: "Start repair" }) as HTMLButtonElement;
  expect(start.disabled).toBe(true);
  fireEvent.click(ui.getByRole("checkbox", { name: /I read this with Bob/ }));
  expect(start.disabled).toBe(true); // one more helper
  fireEvent.click(ui.getByRole("checkbox", { name: "Alice" }));
  fireEvent.click(start);
  await waitFor(() =>
    expect(backend.calls("start_repair")).toEqual([
      { groupId: GROUP_ID, identifier: "03", newContact: "zffrost1bobsnewdevice0000000", helpers: [ALICE, ME] },
    ]),
  );
  await ui.findByText("cyzerepair1abc");
  await emit("repair:complete", { ceremony_id: "r1", group_id: GROUP_ID });
  await ui.findByText(/✓ Repaired: Bob's new device has their share/);
});
