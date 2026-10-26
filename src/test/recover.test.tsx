import { expect, test } from "vitest";
import { fireEvent, waitFor } from "@testing-library/react";
import { emit } from "@tauri-apps/api/event";
import { fakeBackend, GROUP_ID, renderApp } from "./fake";

test("on a new device, a member gets their share back with the repair code the group sends", async () => {
  const backend = fakeBackend({
    export_my_contact: () => ({ name: "me", pubkey: "dd".repeat(32), alias: null, text: "zffrost1mynewdevice", safety_code: "11111 22222", verified: false }),
    read_repair_code: () => ({ group: "Treasury", member: "Carol", server: "frost.example.com" }),
    receive_repair: () => "r2",
  });
  const ui = await renderApp("/groups");
  fireEvent.click(await ui.findByRole("button", { name: "Recover a group" }));
  await ui.findByText("zffrost1mynewdevice");
  fireEvent.change(ui.getByRole("textbox", { name: "Repair code" }), { target: { value: "here: cyzerepair1abc" } });
  await ui.findByText(/as Carol/);
  fireEvent.click(ui.getByRole("button", { name: "Recover" }));
  await waitFor(() => expect(backend.calls("receive_repair")).toEqual([{ code: "here: cyzerepair1abc" }]));
  await emit("repair:complete", { ceremony_id: "r2", group_id: GROUP_ID });
  await ui.findByText(/✓ Recovered/);
});
