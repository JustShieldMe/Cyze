import { afterEach, expect, test, vi } from "vitest";
import { act, fireEvent, waitFor } from "@testing-library/react";
import { fakeBackend, renderApp } from "./fake";

const status = (state: string, scanned: number, extra: object = {}) => ({
  state,
  scanned_height: scanned,
  tip_height: 3_500_000,
  last_synced_ms: Date.now(),
  error: null,
  error_code: null,
  ...extra,
});

const restartButtons = (ui: Awaited<ReturnType<typeof renderApp>>) =>
  ui.queryAllByRole("button", { name: /^(Sync now|Retry now|Restart)$/ });

afterEach(() => {
  vi.useRealTimers();
});

test("an up-to-date wallet offers no sync button: the backend keeps it current", async () => {
  fakeBackend();
  const ui = await renderApp("/");
  await ui.findByText(/Up to date/);
  expect(restartButtons(ui)).toEqual([]);
});

test("a failed sync can be retried straight away", async () => {
  const backend = fakeBackend({
    wallet_sync_status: ({ groupId }) => ({ group_id: groupId, ...status("error", 3_500_000, { error: "connection refused" }) }),
    wallet_sync_now: () => null,
  });
  const ui = await renderApp("/");
  await ui.findByText("Sync failed; retrying");
  fireEvent.click(ui.getByRole("button", { name: "Retry now" }));
  await waitFor(() => expect(backend.calls("wallet_sync_now")).toEqual([{ restart: true }]));
});

test("a sync that stops making progress offers a restart after a minute", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  fakeBackend({
    wallet_sync_status: ({ groupId }) => ({ group_id: groupId, ...status("syncing", 3_400_000) }),
  });
  const ui = await renderApp("/");
  await ui.findByText(/Syncing · 100,000 blocks to go/);
  expect(restartButtons(ui)).toEqual([]);
  await act(async () => {
    vi.advanceTimersByTime(61_000);
  });
  expect(ui.getByRole("button", { name: "Restart" })).toBeTruthy();
});
