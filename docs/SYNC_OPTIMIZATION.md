# Sync optimization — design & roadmap

Status: **the pipelined driver is the standard, only sync path** (validated on
testnet; see "Status: standard driver" below). Its goal is to cut the wall-clock
latency of wallet sync, especially the recovery / large-range case, without
leaving the Ironwood-capable `zcash_client_backend` (ECC) stack. Measured numbers
and what they imply for the next steps are under
[Measurements](#measurements-2026-10-23).

## Why we stay on the ECC stack

We surveyed the open-source Zcash wallets (YWallet/ZKool, Zingo, Cake, ZODL,
Vizor). All but ZODL are light wallets on lightwalletd compact blocks; the
differentiation is entirely in the scan engine.

- **YWallet/ZKool** use **Warp Sync** — the fastest engine — but it is
  Sapling/Orchard-oriented, keeps its own DB schema, and **has no Ironwood
  support**. Adopting it wholesale would fork us off the maintained NU6.3 stack
  for a rewrite: wrong trade for a funds wallet on Ironwood.
- **ZODL** offers a full/hybrid node mode — a privacy feature, not a general
  latency win, and a large architectural add.

So we **port Warp's ideas onto our existing stack** rather than switch engines.

## What we already have (main)

- **Tip-height birthday for new wallets** — a new group starts at the chain tip,
  so it never scans pre-creation history. (The single biggest first-sync win; done.)
- **Subtree-root tree init** — `zcash_client_backend::sync::run` calls
  `update_subtree_roots` (GetSubtreeRoots), so the note-commitment tree is
  initialized without replaying all history.
- **Spend-before-sync ordering** — `run` scans `suggest_scan_ranges()` in
  priority order (ChainTip/Verify first), so the balance surfaces before a full
  catch-up finishes, and the UI polls it every ~5s.
- **Configurable batch size** — `sync_group(batch_size)`, clamped
  `[MIN,MAX]_SYNC_BATCH_SIZE`, persisted via `Settings.sync_batch_size`.

## What's missing (this update)

The upstream `sync::run` is explicit that "block batches are not downloaded in
parallel with scanning." Two levers remain, and both require driving the sync
loop ourselves instead of calling `sync::run`:

1. **Pipelining** — overlap network download with CPU trial-decryption. This is
   Warp's core advantage and the single biggest safe win left. **Implemented and
   now the standard (only) sync driver** (`run_pipelined` in `wallet.rs`). The
   stock `sync::run` path and its on-disk block cache were removed once the
   pipelined driver was validated.
2. **Adaptive batch size** — grow the batch over empty ranges (cheap to scan),
   shrink over dense ranges (expensive), instead of one fixed size for the run.
   **Deliberately deferred**: the pipelined driver keeps the *same* fixed batch
   units as the stock driver so its output is byte-identical and the validation
   gate below is a clean equality check. Adaptive sizing changes the scan units,
   so it lands as a separate follow-up once pipelining is validated and default.

A third lever turned out to be in place already:

3. **Parallel trial decryption.** An earlier version of this doc said decryption
   was single-threaded. It isn't: `scan_cached_blocks` decrypts through
   `zcash_client_backend`'s `BatchRunner`, which fans batches of outputs out to
   the rayon thread pool (`rayon::spawn_fifo` in `scan.rs`). There is nothing to
   enable. The measurements below show decryption threads are mostly idle; the
   limit is single-threaded work elsewhere in the scan.

## Approach: a custom pipelined driver, alongside `sync::run`

Pipelining needs control of the loop, so we add a **custom driver**
(`run_pipelined` / `running_pipelined` in `wallet.rs`) that faithfully reproduces
the upstream `run`/`running` control flow (subtree roots → chain tip → verify
pass → historic ranges), changing only how batches are fed:

- A **producer** task downloads each batch's compact blocks **into memory** and
  the chain-state anchor, then hands `(ScanRange, Vec<CompactBlock>, ChainState)`
  over a **bounded channel** (capacity 2) so download runs up to two batches
  ahead. A cloned tonic client shares the underlying HTTP/2 connection, so this
  adds no new socket.
- The **consumer** (main task) receives a ready batch, wraps its blocks in an
  in-memory `BlockSource` (`MemBlockSource`), and runs `scan_cached_blocks` on it
  (CPU-bound). Scanning is transactional per batch via `put_blocks`, so an
  interrupted or cancelled batch leaves the db consistent at a batch boundary —
  the same guarantee the stock driver gives.
- On a **reorg / continuity error** or a newly-added higher-priority range, the
  consumer breaks, the producer is aborted, and the pass restarts from
  `suggest_scan_ranges` — exactly the upstream `return Ok(true)` → outer-loop
  behavior.

Because each batch is downloaded fresh into memory and never persisted, the
pipelined path **never touches the on-disk `FsCache`** — no file writes, no cache
mutex contended between producer and consumer, and nothing to truncate on a
reorg rewind (only the db is rewound).

**Transparent UTXO refresh is intentionally omitted.** Upstream `running` performs
it under the `transparent-inputs` feature. That feature *is* on in our build (the
`pczt` feature enables it), but a group account is an Orchard-only view key with
no transparent receivers, so the refresh would have nothing to fetch. Omitting it
changes no wallet state and saves a round trip per sync.

Correctness-critical logic (reorg rewind, verify ranges, subtree roots,
chain-tip update, batch splitting) is **ported faithfully** from the upstream
`sync.rs` we depend on; only the download/scan overlap is new. The batch splitter
(`split_scan_range`) has a unit test asserting it produces the exact same units as
the upstream step-7 splitter.

### A note on overlap and the runtime

`scan_cached_blocks` is synchronous and CPU-bound. The consumer used to call it
directly on its Tokio worker, which quietly undermines the overlap: when the
consumer's `recv` frees a channel slot, it wakes the waiting producer into *that
worker's* LIFO slot, the one task Tokio's other workers cannot steal. The next
download then waits until the scan finishes. The consumer now scans inside
`run_blocking` (`block_in_place` on the multi-threaded runtime), which hands the
worker's queue, LIFO slot included, to another thread first. On a single-threaded
runtime (unit tests) the closure just runs inline.

On a fast link this measured neutral (downloads are about 5% of sync time, so
there is little overlap to protect). It matters when download time approaches scan
time: slow or high-latency links, and Tor if that lands.

### Reused connection and cached subtree roots

`sync_group` takes a long-lived `Lightwalletd` connection, and the app keeps one in
`AppState` (dropped after a connection error so the next sync reconnects).
Subtree roots are fetched on a wallet's first sync of the session and then at most
hourly (`SyncOptions::skip_subtree_roots`). Once caught up, the wallet re-syncs
every 10 s, and that cycle used to cost a fresh TLS handshake plus three
subtree-root streams every time.

### Status: standard driver

The pipelined driver was validated on testnet (byte-identical wallet state vs the
stock driver, faster on high-latency links) and is now the **standard, only** sync
path. `sync_group` always calls `run_pipelined`; the `experimental_pipelined_sync`
setting, the stock `zcash_client_backend::sync::run` call, and the on-disk
`FsBlockDb`/`FsCache` block cache it required have been removed. The pipelined
driver streams blocks straight from the network to the scanner in memory.

### Validation gate before default

Before flipping the default to the pipelined driver:

1. A testnet recovery sync (large range) produces a **byte-identical** wallet
   state to the stock driver (same balance, notes, witnesses, scanned height).
2. A reorg is exercised (or simulated) and recovers correctly.
3. A shielded send after a pipelined sync builds, signs, and broadcasts.

## Measurements (2026-10-23)

Taken with the benchmark in `core/examples/sync_bench.rs`. It imports a throwaway
Orchard-only viewing key, derived exactly as a group's is, scans the last N
blocks, then times re-syncs at the tip:

```sh
cd src-tauri
cargo run --release -p frost-app-core --features wallet --example sync_bench -- \
    --network main --blocks 20000            # add --fresh-connection for the old re-sync behaviour
```

Setup: mainnet via `https://zec.rocks:443` (about 50 ms connect), tip about
3.527M, a 12-thread Linux desktop. Every sync also logs a `sync finished: ...` line
with the same breakdown (`SyncStats`).

| Scenario | Result |
|---|---|
| Re-sync at the tip, old behaviour (new connection and subtree roots each time) | about 2,000 ms per cycle, about 1,900 ms of it subtree roots |
| Re-sync at the tip, reused connection and cached roots | about 54 ms per cycle |
| Catch up 10,000 blocks (about 9 days) | 16.8 s, 595 blocks/s |
| Catch up 20,000 blocks | 29.6–32.2 s (620–675 blocks/s), of which scan 29.0–29.9 s, download 1.2–2.8 s |
| Batch size 1,000 / 2,500 / 5,000 / 10,000 (20k blocks) | 33.9 / 30.9 / 31.8 / 31.4 s |
| 256 MiB SQLite page cache (20k blocks) | no change |

What it says:

- **Re-sync overhead was the cheap, large win:** about 2 s down to about 54 ms per
  10-second auto-sync cycle.
- **A catch-up is scan-bound, not network-bound.** Scanning took about 95% of
  the time. Average CPU was about 2.3–2.7 cores out of 12. During the scan, one
  thread (the scan loop) ran at about 84% while each rayon decryption thread
  sat around 15–17%. The critical path is single-threaded work inside
  `scan_cached_blocks` (block parsing, commitment-tree updates, SQLCipher
  writes), not trial decryption.
- **Batch size is not a lever** between 2,500 and 10,000; 5,000 stays the default.
  Adaptive sizing is unlikely to pay off on today's blocks.

### Profile: where the scan's time goes

Sampled in-process with `pprof` (timer signals, so no `perf` permissions
needed) over a 10,000-block mainnet sync:

| Share of samples | Where |
|---|---|
| ~78% | Orchard note-commitment tree hashing (`MerkleHashOrchard::combine`, i.e. Sinsemilla over Pasta) |
| ~56% | of that: single-threaded `shardtree` insertion inside `put_blocks` / `update_tree` |
| ~13% | of that: `sinsemilla::HashDomain::new`, because upstream `orchard` 0.15.5 rebuilds the MerkleCRH domain (a hash-to-curve) on every hash |
| ~13% | trial decryption (already parallel) |
| <1% | Sapling tree hashing; SQLite/SQLCipher barely registers |

So the bottleneck is Orchard tree hashing, not decryption, the database, or the
network.

## Option: the Zakura crate stack (as used by Vizor)

Vizor (Keplr's Zcash wallet, Apache-2.0, `chainapsis/vizor-wallet`) runs its sync
on Rust with the same overall shape as ours: compact blocks fed to
`scan_cached_blocks`, one batch prefetched ahead. Its notable differences:

- It builds on **Zakura**, published forks of the whole Zcash crate family
  (`zakura-core/common` for the crypto and protocol crates,
  `zakura-core/wallet-libraries` for `zakura-client-backend` / `-sqlite` /
  `-pczt`), with library names kept upstream so `use` paths don't change.
- It ships compiled tree-state checkpoints, so most batches skip `get_tree_state`,
  and checks that downloaded blocks actually follow that state (re-fetching by
  hash if not).
- Smaller batches (2,000 on desktop) and 100-block batches inside mainnet's
  2022–23 spam range (1.71M–2.05M), matching ECC's mobile SDK.
- Private queries (PIR) for transaction enhancement and transparent recovery,
  and Tor.

Zakura's Orchard fork caches the MerkleCRH domain and, with its
`weighted-merkle` feature, hashes through precomputed tables over 10-bit words,
plus a batched `combine_batch`. Micro-benchmark on identical inputs (outputs
byte-identical across all 20,000 hashes):

| Implementation | Per node hash | Speed-up |
|---|---|---|
| `orchard` 0.15.5 `combine` (what we use) | 87 µs | 1× |
| upstream with the domain cached (a one-line patch) | 72 µs | 1.2× |
| `zakura-orchard` 4.0.0 `combine` (drop-in) | 12 µs | 7.3× |
| `zakura-orchard` 4.0.0 `combine_batch` | 5.2 µs | 16.8× |

With tree hashing at ~78% of scan time, the drop-in hash alone predicts a catch-up
roughly 3× faster (20,000 blocks: ~30 s to ~10 s); batching in the tree code
would go further. A cached-domain patch upstream is not worth carrying on its
own.

Adopting it means moving the wallet onto the Zakura crate family (a different
maintainer and release line from ECC's, currently release candidates for the
wallet layer), which reverses the "stay on the ECC stack" decision above. It
needs a scoping spike before any decision: port `wallet.rs` to
`zakura-client-backend`, check the FROST path (`zakura-orchard` keeps
`unstable-frost`; the signature crosses as bytes), compare wallet state against
the current stack over the same range, and measure with `sync_bench`.

## Background sync service

`src-tauri/src/sync_service.rs` runs the active wallet's sync in the backend
from unlock to lock. Before it, the wallet page drove sync on a 10 s timer, so
nothing synced unless that page was open. Each pass sets up the account if
needed, checks the chain tip (one RPC on the shared connection), and syncs only
when there is something new. Failures back off from 10 s to 5 min. "Sync Now", a
wallet switch, a network change, or a finished signing ceremony wake it at
once. The UI follows `wallet:sync` / `wallet:updated` events instead of polling.
`tests/sync_service_live.rs` (ignored by default; needs the network) runs it end
to end against mainnet after a real DKG.

New groups record their wallet birthday at key generation (the chain tip then,
per network), so a member who first opens the wallet weeks later still scans
from the group's creation instead of from the current tip.

## Sequencing

1. **[done] Custom pipelined driver** (prefetch download while scanning);
   `run_pipelined` in `wallet.rs`.
2. **[done] Testnet validation** against the stock driver; it is now the only
   driver (see the validation gate above).
3. **[done] Reused connection, cached subtree roots, scan off the async worker,
   per-sync `SyncStats`, and the `sync_bench` benchmark.**
4. **[done] Profile the scan.** Orchard tree hashing is ~78% (see above).
5. **[done] Background sync service and birthdays recorded at key generation.**
6. **[decision needed] Zakura spike**: port to the Zakura wallet libraries and
   measure (expected ~3× faster catch-up).
7. **[small, from Vizor] Spam-range batches and a block/tree-state consistency
   check.** Only deep mainnet rescans cross the 2022–23 spam range.
8. **[deprioritized] Adaptive batch size.** The batch-size sweep shows little to
   gain. ~~Parallel trial decryption~~ is already in place upstream.
9. **[optional, infra] Zaino indexer** — evaluate a Rust indexer (Zingo's path)
   in place of stock lightwalletd for richer per-request data. Composes with the
   above; not a wallet rewrite.

## Explicit non-goals

- No wholesale swap to Warp/ZKool's engine (no Ironwood support; own DB).
- No full/hybrid-node mode (privacy feature, not latency; large surface).
