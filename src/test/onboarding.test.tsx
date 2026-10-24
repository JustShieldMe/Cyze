import { expect, test } from "vitest";
import { fireEvent } from "@testing-library/react";
import { fakeBackend, renderApp } from "./fake";

test("a new user creates a keystore, saves the recovery code, and lands on Home", async () => {
  let created = false;
  const backend = fakeBackend({
    keystore_status: () => ({ exists: created, unlocked: created, recovery_enabled: created }),
    create_keystore: () => {
      created = true;
      return "abandon ability able about above absent absorb abstract absurd abuse access accident";
    },
    set_username: () => null,
    list_groups: () => [],
    get_active_wallet: () => null,
  });
  const ui = await renderApp("/");

  await ui.findByText("Create keystore");
  fireEvent.change(ui.getByPlaceholderText(/shown to you in groups/), { target: { value: "Miguel" } });
  const [passphrase, confirm] = ui.container.querySelectorAll<HTMLInputElement>('input[type="password"]');
  fireEvent.change(passphrase, { target: { value: "correct horse battery" } });
  fireEvent.change(confirm, { target: { value: "correct horse battery" } });
  fireEvent.click(ui.getByRole("button", { name: "Create" }));

  // The recovery code is shown once, behind an acknowledgement.
  await ui.findByText("Save your recovery code");
  expect(ui.getByText("abstract")).toBeTruthy();
  const proceed = ui.getByRole("button", { name: /I have saved my recovery code/ }) as HTMLButtonElement;
  expect(proceed.disabled).toBe(true);
  fireEvent.click(ui.getByRole("checkbox"));
  fireEvent.click(proceed);

  // Home offers the two ways in.
  await ui.findByText("Start a shared wallet");
  expect(ui.getByText("Join with an invite")).toBeTruthy();
  expect(backend.calls("create_keystore")).toEqual([{ passphrase: "correct horse battery" }]);
  expect(backend.calls("set_username")).toEqual([{ username: "Miguel" }]);
});

test("mismatched passphrases are caught before anything is created", async () => {
  const backend = fakeBackend({ keystore_status: () => ({ exists: false, unlocked: false, recovery_enabled: false }) });
  const ui = await renderApp("/unlock");
  await ui.findByText("Create keystore");
  const [passphrase, confirm] = ui.container.querySelectorAll<HTMLInputElement>('input[type="password"]');
  fireEvent.change(passphrase, { target: { value: "correct horse battery" } });
  fireEvent.change(confirm, { target: { value: "correct horse batteyr" } });
  fireEvent.click(ui.getByRole("button", { name: "Create" }));
  await ui.findByText("Passphrases do not match");
  expect(backend.calls("create_keystore")).toEqual([]);
});
