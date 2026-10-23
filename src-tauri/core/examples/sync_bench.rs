//! Sync benchmark: times a real wallet sync against a lightwalletd server.
//!
//! Imports a throwaway view-only account (an Orchard-only viewing key derived
//! exactly as Cyze derives a group's, so trial decryption costs the same), scans
//! the last N blocks, then times a few re-syncs at the tip. It reports where the
//! time went and whether the sync was limited by the network or by scanning.
//!
//! ```sh
//! cd src-tauri
//! cargo run --release -p frost-app-core --features wallet --example sync_bench -- \
//!     --network main --blocks 10000
//! ```
//!
//! Run with `--release`: debug builds scan many times slower. Per-batch timings
//! are logged at debug level: add `RUST_LOG=frost_app_core=debug`.

use std::time::Instant;

use frost_app_core::wallet::{self, Lightwalletd, SyncOptions, SyncStats, WalletNetwork};
use rand::RngCore;
use tokio_util::sync::CancellationToken;

const USAGE: &str = "\
Usage: sync_bench [options]
  --network main|test|regtest   network to sync (default: main)
  --url URL                     lightwalletd endpoint (default: the network's default)
  --blocks N                    scan the last N blocks (default: 10000)
  --batch-size N                blocks per batch (default: the app's default)
  --resyncs K                   re-syncs to time once caught up (default: 3)
  --fresh-connection            reconnect and refetch subtree roots on every re-sync
                                (how the app behaved before connections were reused)";

struct Args {
    network: WalletNetwork,
    url: Option<String>,
    blocks: u64,
    batch_size: Option<u32>,
    resyncs: u32,
    fresh_connection: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        network: WalletNetwork::Main,
        url: None,
        blocks: 10_000,
        batch_size: None,
        resyncs: 3,
        fresh_connection: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--network" => {
                args.network = match value()?.as_str() {
                    "main" => WalletNetwork::Main,
                    "test" => WalletNetwork::Test,
                    "regtest" => WalletNetwork::from_str("regtest"),
                    other => return Err(format!("unknown network '{other}'")),
                }
            }
            "--url" => args.url = Some(value()?),
            "--blocks" => args.blocks = value()?.parse().map_err(|e| format!("--blocks: {e}"))?,
            "--batch-size" => {
                args.batch_size = Some(value()?.parse().map_err(|e| format!("--batch-size: {e}"))?)
            }
            "--resyncs" => args.resyncs = value()?.parse().map_err(|e| format!("--resyncs: {e}"))?,
            "--fresh-connection" => args.fresh_connection = true,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(args)
}

/// A random, valid Orchard `ak` (from a random spending key), turned into a
/// viewing key the way the app does it for a group.
fn throwaway_ufvk(network: WalletNetwork) -> String {
    loop {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let Some(sk) = Option::<orchard::keys::SpendingKey>::from(
            orchard::keys::SpendingKey::from_bytes(seed),
        ) else {
            continue;
        };
        let fvk = orchard::keys::FullViewingKey::from(&sk).to_bytes();
        let ak: [u8; 32] = fvk[..32].try_into().expect("32 bytes");
        if let Ok(keys) = frost_app_core::zcash::derive_orchard_keys(&ak, None, network.network_type()) {
            return keys.ufvk;
        }
    }
}

fn ms(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Which leg limits throughput: the one that was busy longer. (The wait figures
/// show how well the legs overlapped, not which is slower: with only a couple of
/// batches the downloader never fills the queue, so it never waits.)
fn bottleneck(s: &SyncStats) -> &'static str {
    if s.batches == 0 {
        "n/a (nothing scanned)"
    } else if s.scan_ms >= s.download_ms {
        "scanning (CPU/db): it took longer than downloading"
    } else {
        "network: downloading took longer than scanning"
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    if std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_target(false)
            .init();
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}\n");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), frost_app_core::CoreError> {
    let network = args.network;
    let url = args
        .url
        .clone()
        .unwrap_or_else(|| network.default_lightwalletd().to_string());
    let dir = tempfile::tempdir()?;
    let mut db_key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut db_key);
    let ufvk = throwaway_ufvk(network);
    let group = "bench";
    let opts = |skip_subtree_roots| SyncOptions {
        batch_size: args.batch_size,
        skip_subtree_roots,
    };

    let info = wallet::lightwalletd_info(&url).await?;
    let tip = info.block_height;
    let birthday = tip.saturating_sub(args.blocks);
    println!("Cyze sync benchmark: {} via {url}", network.as_str());
    println!(
        "server {} {} at height {}; scanning from {} ({} blocks); {} CPU threads",
        info.vendor,
        info.version,
        ms(tip),
        ms(birthday),
        ms(args.blocks),
        std::thread::available_parallelism().map_or(0, |n| n.get()),
    );

    let t = Instant::now();
    let scan_from =
        wallet::init_group_account(dir.path(), group, network, &ufvk, &url, &db_key, Some(birthday))
            .await?;
    println!("\naccount setup      {:>9} ms (first block to scan: {})", ms(t.elapsed().as_millis() as u64), ms(scan_from));

    let t = Instant::now();
    let mut conn = Lightwalletd::connect(&url).await?;
    println!("connect            {:>9} ms", ms(t.elapsed().as_millis() as u64));

    let cancel = CancellationToken::new();
    let s = wallet::sync_group(dir.path(), group, network, &conn, &db_key, opts(false), &cancel).await?;
    println!(
        "initial sync       {:>9} ms  {} blocks in {} batches, {:.0} blocks/s",
        ms(s.total_ms),
        ms(s.blocks),
        s.batches,
        s.blocks_per_sec()
    );
    println!("  subtree roots    {:>9} ms", ms(s.subtree_roots_ms));
    println!("  download         {:>9} ms  (downloader waited {} ms on the scanner)", ms(s.download_ms), ms(s.download_wait_ms));
    println!("  scan             {:>9} ms  (scanner waited {} ms on the network)", ms(s.scan_ms), ms(s.scan_wait_ms));
    println!("  bottleneck       {}", bottleneck(&s));

    if args.resyncs > 0 {
        println!(
            "\nre-syncs at the tip ({})",
            if args.fresh_connection {
                "new connection + subtree roots each time"
            } else {
                "reused connection, cached subtree roots"
            }
        );
        let mut total = 0;
        for i in 1..=args.resyncs {
            let t = Instant::now();
            if args.fresh_connection {
                conn = Lightwalletd::connect(&url).await?;
            }
            let s = wallet::sync_group(
                dir.path(),
                group,
                network,
                &conn,
                &db_key,
                opts(!args.fresh_connection),
                &cancel,
            )
            .await?;
            let elapsed = t.elapsed().as_millis() as u64;
            total += elapsed;
            println!(
                "  #{i:<3}            {:>9} ms  ({} blocks, subtree roots {} ms)",
                ms(elapsed),
                s.blocks,
                ms(s.subtree_roots_ms)
            );
        }
        println!("  average          {:>9} ms", ms(total / u64::from(args.resyncs)));
    }
    Ok(())
}
