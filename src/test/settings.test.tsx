import { expect, test } from "vitest";
import { fireEvent } from "@testing-library/react";
import { fakeBackend, renderApp } from "./fake";

test("every Settings tab opens", async () => {
  const backend = fakeBackend({
    get_settings: () => ({ server_url: "frost.example.com", trusted_certs: {}, advanced_mode: true }),
    third_party_notices: () => "Cyze: third-party notices\n\nMIT License …",
    get_logs: () => ["sync: 20000 blocks in 9.7 s"],
    sidecar_status: () => ({ running: false, port: null, url: null, cert_fingerprint: null, lan_addresses: [] }),
    tunnel_status: () => ({ running: false, public_url: null, port: null }),
    tailscale_status: () => ({ installed: false, available: false, serving: false, public_url: null, port: null, dns_name: null, detail: "" }),
    "plugin:app|version": () => "0.3.0-rc.1",
  });
  const ui = await renderApp("/settings");
  await ui.findByText("I'm organizing");

  fireEvent.click(ui.getByRole("link", { name: "Wallet" }));
  await ui.findByRole("heading", { name: /^(Network|Sync server)$/ });
  // No server chosen: the app shows which one it picked.
  await ui.findByText(/Using eu.zec.rocks, which answered in 48 ms/);
  expect((ui.getByRole("combobox") as HTMLSelectElement).value).toBe("auto");

  fireEvent.click(ui.getByRole("link", { name: "Security" }));
  await ui.findByText("Key backup");
  expect(ui.getByRole("button", { name: "Change passphrase" })).toBeTruthy();

  fireEvent.click(ui.getByRole("link", { name: "Advanced" }));
  await ui.findByText("Show advanced options");
  await ui.findByText(/sync: 20000 blocks/);

  fireEvent.click(ui.getByRole("link", { name: "About" }));
  await ui.findByText("Cyze 0.3.0-rc.1");
  fireEvent.click(ui.getByRole("button", { name: "Show licenses" }));
  await ui.findByText(/MIT License/);
  expect(backend.calls("third_party_notices")).toHaveLength(1);
});
