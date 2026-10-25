import { expect, test } from "vitest";
import { fireEvent, waitFor } from "@testing-library/react";
import { ALICE, fakeBackend, GROUP_ID, ME, renderApp } from "./fake";

const poll = (myVote: number | null) => ({
  group_id: GROUP_ID,
  poll_id: "p1",
  author: ME,
  title: "Hire an auditor?",
  details: "Two quotes so far.",
  options: ["Yes", "No"],
  created_at: Math.floor(Date.now() / 1000) - 60,
  closes_at: Math.floor(Date.now() / 1000) + 3 * 86_400,
  closed: false,
  votes: myVote === null ? { [ALICE]: 0 } : { [ALICE]: 0, [ME]: myVote },
  tally: myVote === null ? [1, 0] : [2, 0],
  my_vote: myVote,
  members: 3,
  threshold: 2,
  agreed: myVote === 0 ? 0 : null,
  community: null,
});

test("a member asks the group a question, then answers it", async () => {
  let created = false;
  let voted = false;
  const backend = fakeBackend({
    list_polls: () => (created ? [poll(voted ? 0 : null)] : []),
    create_poll: () => {
      created = true;
      return "p1";
    },
    vote_poll: () => {
      voted = true;
      return null;
    },
  });
  const ui = await renderApp("/votes");
  await ui.findByText(/No open polls/);
  fireEvent.click(ui.getByRole("button", { name: "New poll" }));
  fireEvent.change(ui.getByPlaceholderText(/Hire an auditor before launch/), { target: { value: "Hire an auditor?" } });
  fireEvent.click(ui.getByRole("button", { name: "Add an option" }));
  fireEvent.change(ui.getByPlaceholderText("Option 3"), { target: { value: "Later" } });
  fireEvent.click(ui.getByRole("button", { name: "Remove option 3" }));
  fireEvent.click(ui.getByRole("button", { name: "Ask the group" }));
  await waitFor(() =>
    expect(backend.calls("create_poll")).toEqual([
      { args: { group_id: GROUP_ID, title: "Hire an auditor?", details: "", options: ["Yes", "No"], open_for_secs: 3 * 86_400 } },
    ]),
  );

  // The poll shows Alice's answer; this member picks Yes and votes.
  await ui.findByText("Two quotes so far.");
  await ui.findByText("Alice");
  expect(ui.getByText(/1 of 3 voted · 2 must agree/)).toBeTruthy();
  fireEvent.click(ui.getByRole("radio", { name: /Yes/ }));
  fireEvent.click(ui.getByRole("button", { name: "Vote" }));
  await waitFor(() => expect(backend.calls("vote_poll")).toEqual([{ groupId: GROUP_ID, pollId: "p1", choice: 0 }]));
  await ui.findByText("agreed");
  expect(ui.getByRole("button", { name: "Change vote" })).toBeTruthy();
});

test("community votes show what's open, and a group can decide together", async () => {
  const now = Math.floor(Date.now() / 1000);
  const backend = fakeBackend({
    community_votes: () => ({
      open: [
        {
          id: "ab12",
          title: "NU8 scope",
          description: "Which features go into NU8.",
          discussion_url: "https://forum.zcashcommunity.com/t/nu8",
          snapshot_height: 3_540_000,
          vote_end_time: now + 5 * 86_400,
          created_at: now - 86_400,
          status: "active",
          test: false,
          proposals: [
            { id: 1, title: "Memo bundles", description: "", options: ["Support", "Oppose"], results: null },
            { id: 2, title: "Zcash Shielded Assets", description: "", options: ["Support", "Oppose", "Abstain"], results: null },
          ],
        },
      ],
      recent: [
        {
          id: "cd34",
          title: "NU7 Scope",
          description: "",
          discussion_url: null,
          snapshot_height: 3_480_000,
          vote_end_time: now - 30 * 86_400,
          created_at: now - 50 * 86_400,
          status: "finalized",
          test: false,
          proposals: [{ id: 1, title: "Ironwood", description: "", options: ["Support", "Oppose"], results: [300, 100] }],
        },
      ],
    }),
    poll_community_round: () => 1,
  });
  const ui = await renderApp("/votes");
  await ui.findByText("NU8 scope");
  expect(ui.getByText(/2 proposals · weight from block 3,540,000/)).toBeTruthy();
  expect(ui.getByText("75%")).toBeTruthy(); // NU7 Scope's result, in "Recent results"
  fireEvent.click(ui.getByRole("button", { name: "Decide together" }));
  fireEvent.click(ui.getByRole("checkbox", { name: "Memo bundles" }));
  fireEvent.click(ui.getByRole("button", { name: "Ask the group (1)" }));
  await waitFor(() =>
    expect(backend.calls("poll_community_round")).toEqual([{ groupId: GROUP_ID, roundId: "ab12", proposalIds: [2] }]),
  );

});
