# Voting in Cyze

**Votes** in the sidebar has two kinds of vote:

- **Group polls.** Ask the members of one of your groups a question and see
  where everyone stands. Use it to signal, or to agree before someone sets up a
  payment.
- **Zcash community votes.** See what Zcash coinholders are voting on through
  [Shielded Vote](https://valargroup.gitbook.io/shielded-vote-docs), and agree
  as a group on how your wallet would vote.

Neither kind moves funds or puts anything on chain.

A red **!** next to **Votes** means a poll is waiting for your answer. If
Cyze isn't in front when a new poll arrives, you also get a system
notification.

---

## Group polls

A poll belongs to one group. Every member of that group can answer it, and
nobody outside it can see it.

### Ask a question

1. Open **Votes** and click **New poll**.
2. If you're in more than one group, choose the **Group** to ask.
3. Write the **Question** and, if it helps, some **Details**: the background,
   links, amounts.
4. Set the **Options**. A poll starts with *Yes* and *No*; rename them,
   **Add an option** (up to 10), or **Remove** one (at least 2 stay).
5. Choose how long it stays **Open for**: 1 day, 3 days, 1 week or 2 weeks.
6. Click **Ask the group**.

The poll appears on every online member's Votes page within about 10
seconds. Members who are offline get it later (see
[If you were offline](#if-you-were-offline)).

You can't edit a poll once it's sent. To change the question or the options,
close it and ask again.

| Field | Limit |
|---|---|
| Question | 200 characters |
| Details | 2,000 characters |
| Options | 2 to 10, each up to 100 characters |

### Answer a poll

1. Pick an option.
2. Click **Vote**.

Until the poll closes, you can pick another option and click **Change vote**.
Only your latest answer counts. You can change your answer, but not withdraw
it.

Under the options, a poll shows:

- **how many members have voted**, and how many must agree, for example
  "2 of 3 voted · 2 must agree";
- **your current choice**, for example "you chose Yes";
- **who chose each option**, next to that option;
- **a bar per option** showing its share of the group.

### When an option is agreed

An option is marked **agreed** once as many members choose it as the group
needs to approve a payment. In a 2-of-3 group, that's two members.

- "Agreed" signals what the group wants. It doesn't do anything by itself: a
  payment still needs its own approvals.
- If no option reaches that number, nothing is agreed.
- If someone changes their vote, the count follows it. An option can lose
  "agreed" while the poll is still open.

### Closing

A poll closes when its time is up. The member who asked can also close it
early:

1. Click **Close poll**.
2. Confirm.

Nobody can vote after that. A vote sent before the poll closed still counts,
even if it reaches other members later.

Closed polls move to **Closed polls** at the bottom of the page, with their
final counts.

### Who sees what

- **Members see every answer.** Everyone in the group sees who chose what.
  Polls are not anonymous.
- **Nobody else can read polls.** They travel through the group's coordination
  server (the same one approvals use), encrypted end to end between members.
  The server only relays messages it can't read.
- **No one can vote for you.** Each member's app sends only the records its
  member wrote, and the encryption proves who sent them.
- **On your device,** polls are stored encrypted, like the rest of your data.
- **Removing a group** from Cyze deletes its polls from your device.

### If you were offline

A poll, a vote or an early close reaches members who are online straight
away. For a member who was offline:

- **The server holds messages for a while.** It keeps them for about a day
  after the last activity, and their app collects them the next time they
  connect.
- **The apps catch up with each other.** Each app asks the other members for
  anything it missed, shortly after you unlock Cyze and then every few hours.
  Every app answers by resending its own member's polls and votes.

Each member's polls and votes only come from that member's app. So you get
them once that member's app has been online too.

---

## Zcash community votes

Zcash coinholders vote on proposals for the network through Shielded Vote:

- **Weight** is the balance a wallet held at the round's snapshot block.
- **Ballots are private.**
- **Results are published** once the round is counted.

The **Zcash community votes** card on the Votes page shows the rounds for the
network Cyze is on:

| Network | Rounds shown |
|---|---|
| Mainnet | Production rounds |
| Testnet | Staging (test) rounds |
| Local devnet | None |

### What a round shows

- **Its title, description and status:**
  - "opens soon" before voting starts;
  - "closes in 3 days" while it's open;
  - "counting" once it has closed;
  - "closed 12 days ago" when it's done.
- **How many proposals it has**, and the **block** whose balances give each
  wallet its weight.
- **A Discussion link**, when the round has a forum thread.
- **Recent results:** each proposal's options, with the share of the vote that
  went to each.

Cyze checks for rounds at most every 10 minutes. If you turned on Tor under
**Settings → Wallet → Privacy**, it checks through Tor. The requests are the
same for every user and say nothing about your wallet.

### Decide together

**Decide together** turns a round's proposals into group polls, so the group
can agree on how its wallet would vote:

1. Under an open round, click **Decide together**.
2. If you have more than one Zcash wallet group, choose the group.
3. Tick the proposals to ask about. The first five are ticked to start.
4. Click **Ask the group**.

Cyze then creates one poll per proposal:

- **Options:** the proposal's own.
- **Closing time:** an hour before the round closes.
- **Badge:** **Zcash community vote**, so these polls stand out from the
  group's own.

They work like any other group poll. The **agreed** option is how the group
would vote.

### Casting the group's vote

Casting the group's vote with its balance isn't in Cyze yet. It will come in a
later version, once it's been tested on a live round. Until then, use the
group's agreed answers to vote with other tools, or keep them as a record of
where the group stood.

---

## Questions

**Someone says they asked a poll, but I don't see it.**
Check that Cyze is connected to the group's coordination server
(**Settings → Connection**): polls use the same server as approvals. Then
lock and unlock Cyze: shortly after an unlock, it asks the other members for
anything it missed. The member who asked needs Cyze open for their reply to
come through.

**Can I vote without the others seeing my choice?**
No. Group polls are for members who already share a wallet, so answers are
open within the group. Only people outside the group can't see them.

**Does voting cost anything, or touch the wallet?**
No. Polls are messages between members. They don't spend funds, need fees, or
put anything on chain.

**Can I poll a group that isn't a Zcash wallet?**
Yes, any group can have polls. **Decide together** needs a Zcash wallet group,
since the vote would be cast with the wallet's balance.
