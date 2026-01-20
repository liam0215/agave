#![allow(clippy::arithmetic_side_effects)]

use {
    clap::{crate_description, crate_name, App, Arg, ArgMatches},
    crossbeam_channel::{bounded, Receiver, Sender},
    log::{debug, error, info, warn},
    solana_bench_tps_rate::{
        confirmation::{spawn_confirmation_tracker, ConfirmationTrackerConfig},
        rate_limiter::RateLimiter,
        transaction::TransactionGenerator,
    },
    solana_clap_utils::input_validators::{is_keypair, is_url_or_moniker, is_within_range},
    solana_cli_config::{ConfigInput, CONFIG_FILE},
    solana_client::connection_cache::ConnectionCache,
    solana_rpc_client::rpc_client::RpcClient,
    solana_sdk::{
        commitment_config::CommitmentConfig,
        hash::Hash,
        native_token::Sol,
        signature::{read_keypair_file, Keypair, Signer},
        system_instruction,
        transaction::Transaction,
    },
    solana_tps_client::TpsClient,
    solana_tpu_client::tpu_client::{TpuClient, TpuClientConfig},
    std::{
        net::{IpAddr, Ipv4Addr, SocketAddr},
        process::exit,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc, RwLock,
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    },
};

/// Configuration for the rate-limited benchmark.
#[derive(Debug)]
struct Config {
    json_rpc_url: String,
    websocket_url: String,
    id: Keypair,
    target_tps: u64,
    duration_secs: Option<u64>,
    tx_count: Option<u64>,
    use_tpu_client: bool,
    use_quic: bool,
    compute_unit_price: Option<u64>,
    bind_address: IpAddr,
    commitment_config: CommitmentConfig,
    num_keypairs: usize,
    confirmation_timeout_secs: u64,
    faucet_addr: Option<SocketAddr>,
    client_node_id: Option<Keypair>,
    tpu_connection_pool_size: usize,
    num_lamports_per_account: u64,
    num_threads: usize,
    /// Warmup period in seconds (transactions sent but not tracked)
    warmup_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            json_rpc_url: ConfigInput::default().json_rpc_url,
            websocket_url: ConfigInput::default().websocket_url,
            id: Keypair::new(),
            target_tps: 100,
            duration_secs: Some(60),
            tx_count: None,
            use_tpu_client: true,
            use_quic: true,
            compute_unit_price: None,
            bind_address: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            commitment_config: CommitmentConfig::confirmed(),
            num_keypairs: 100,
            confirmation_timeout_secs: 60,
            faucet_addr: None,
            client_node_id: None,
            tpu_connection_pool_size: 4,
            num_lamports_per_account: solana_sdk::native_token::LAMPORTS_PER_SOL,
            num_threads: 1,
            warmup_secs: 0,
        }
    }
}

fn build_args<'a>(version: &'_ str) -> App<'a, '_> {
    App::new(crate_name!())
        .about(crate_description!())
        .version(version)
        .arg({
            let arg = Arg::with_name("config_file")
                .short("C")
                .long("config")
                .value_name("FILEPATH")
                .takes_value(true)
                .global(true)
                .help("Configuration file to use");
            if let Some(ref config_file) = *CONFIG_FILE {
                arg.default_value(config_file)
            } else {
                arg
            }
        })
        .arg(
            Arg::with_name("json_rpc_url")
                .short("u")
                .long("url")
                .value_name("URL_OR_MONIKER")
                .takes_value(true)
                .global(true)
                .validator(is_url_or_moniker)
                .help(
                    "URL for Solana's JSON RPC or moniker (or their first letter): \
                       [mainnet-beta, testnet, devnet, localhost]",
                ),
        )
        .arg(
            Arg::with_name("websocket_url")
                .long("ws")
                .value_name("URL")
                .takes_value(true)
                .global(true)
                .help("WebSocket URL for the solana cluster"),
        )
        .arg(
            Arg::with_name("authority")
                .short("a")
                .long("authority")
                .alias("keypair")
                .value_name("PATH")
                .takes_value(true)
                .validator(is_keypair)
                .help("File containing a client authority keypair to fund participating accounts"),
        )
        .arg(
            Arg::with_name("rate")
                .short("r")
                .long("rate")
                .value_name("TPS")
                .takes_value(true)
                .required(true)
                .validator(|s| is_within_range(s, 1..))
                .help("Target transactions per second"),
        )
        .arg(
            Arg::with_name("duration")
                .long("duration")
                .value_name("SECS")
                .takes_value(true)
                .help("Test duration in seconds (default: 60)"),
        )
        .arg(
            Arg::with_name("tx_count")
                .long("tx-count")
                .alias("tx_count")
                .value_name("COUNT")
                .takes_value(true)
                .help("Total number of transactions to send (if set, overrides --duration)"),
        )
        .arg(
            Arg::with_name("threads")
                .short("t")
                .long("threads")
                .value_name("NUM")
                .takes_value(true)
                .validator(|s| is_within_range(s, 1..))
                .help("Number of sender threads (default: 1). Higher values allow higher TPS."),
        )
        .arg(
            Arg::with_name("use_rpc_client")
                .long("use-rpc-client")
                .takes_value(false)
                .help("Submit transactions with RpcClient instead of TpuClient"),
        )
        .arg(
            Arg::with_name("tpu_disable_quic")
                .long("tpu-disable-quic")
                .takes_value(false)
                .help("Do not submit transactions via QUIC; only affects TpuClient"),
        )
        .arg(
            Arg::with_name("tpu_connection_pool_size")
                .long("tpu-connection-pool-size")
                .takes_value(true)
                .help("Controls the connection pool size per remote address; only affects TpuClient"),
        )
        .arg(
            Arg::with_name("compute_unit_price")
                .long("compute-unit-price")
                .value_name("PRICE")
                .takes_value(true)
                .validator(|s| is_within_range(s, 0..))
                .help("Sets compute-unit-price for transfer transactions (micro-lamports)"),
        )
        .arg(
            Arg::with_name("num_keypairs")
                .long("num-keypairs")
                .value_name("COUNT")
                .takes_value(true)
                .validator(|s| is_within_range(s, 2..))
                .help("Number of keypairs to generate and fund (default: 100)"),
        )
        .arg(
            Arg::with_name("num_lamports_per_account")
                .long("num-lamports-per-account")
                .value_name("LAMPORTS")
                .takes_value(true)
                .help("Number of lamports per account (default: 1 SOL)"),
        )
        .arg(
            Arg::with_name("confirmation_timeout")
                .long("confirmation-timeout")
                .value_name("SECS")
                .takes_value(true)
                .help("Timeout for considering a transaction dropped (default: 60)"),
        )
        .arg(
            Arg::with_name("commitment_config")
                .long("commitment-config")
                .takes_value(true)
                .possible_values(&["processed", "confirmed", "finalized"])
                .default_value("confirmed")
                .help("Block commitment config for confirmations"),
        )
        .arg(
            Arg::with_name("faucet")
                .short("d")
                .long("faucet")
                .value_name("HOST:PORT")
                .takes_value(true)
                .help("Faucet address to request SOL for funding accounts"),
        )
        .arg(
            Arg::with_name("bind_address")
                .long("bind-address")
                .value_name("HOST")
                .takes_value(true)
                .validator(solana_net_utils::is_host)
                .requires("client_node_id")
                .help("IP address to use with connection cache"),
        )
        .arg(
            Arg::with_name("client_node_id")
                .long("client-node-id")
                .value_name("PATH")
                .takes_value(true)
                .requires("json_rpc_url")
                .validator(is_keypair)
                .help("File containing the node identity keypair of a validator with active stake. \
                       This allows communicating with network using staked connection"),
        )
        .arg(
            Arg::with_name("entrypoint")
                .short("n")
                .long("entrypoint")
                .value_name("HOST:PORT")
                .takes_value(true)
                .help("Cluster entrypoint (currently unused, for CLI compatibility)"),
        )
        .arg(
            Arg::with_name("warmup")
                .long("warmup")
                .value_name("SECS")
                .takes_value(true)
                .help("Warmup period in seconds. Transactions are sent but not tracked for statistics (default: 0)"),
        )
}

fn parse_args(matches: &ArgMatches) -> Result<Config, &'static str> {
    let mut args = Config::default();

    let config = if let Some(config_file) = matches.value_of("config_file") {
        solana_cli_config::Config::load(config_file).unwrap_or_default()
    } else {
        solana_cli_config::Config::default()
    };

    let (_, json_rpc_url) = ConfigInput::compute_json_rpc_url_setting(
        matches.value_of("json_rpc_url").unwrap_or(""),
        &config.json_rpc_url,
    );
    args.json_rpc_url = json_rpc_url;

    let (_, websocket_url) = ConfigInput::compute_websocket_url_setting(
        matches.value_of("websocket_url").unwrap_or(""),
        &config.websocket_url,
        matches.value_of("json_rpc_url").unwrap_or(""),
        &config.json_rpc_url,
    );
    args.websocket_url = websocket_url;

    let (_, id_path) = ConfigInput::compute_keypair_path_setting(
        matches.value_of("authority").unwrap_or(""),
        &config.keypair_path,
    );
    if let Ok(id) = read_keypair_file(&id_path) {
        args.id = id;
    } else if matches.is_present("authority") {
        return Err("could not parse authority keypair path");
    }

    if let Some(rate) = matches.value_of("rate") {
        args.target_tps = rate.parse().map_err(|_| "can't parse rate")?;
    }

    if let Some(duration) = matches.value_of("duration") {
        args.duration_secs = Some(duration.parse().map_err(|_| "can't parse duration")?);
    }

    if let Some(tx_count) = matches.value_of("tx_count") {
        args.tx_count = Some(tx_count.parse().map_err(|_| "can't parse tx-count")?);
    }

    // If tx_count is set, it takes precedence over duration
    if args.tx_count.is_some() {
        args.duration_secs = None;
    } else if args.duration_secs.is_none() {
        // Default to 60 seconds if neither specified
        args.duration_secs = Some(60);
    }

    if let Some(threads) = matches.value_of("threads") {
        args.num_threads = threads.parse().map_err(|_| "can't parse threads")?;
    }

    if matches.is_present("use_rpc_client") {
        args.use_tpu_client = false;
    }

    if matches.is_present("tpu_disable_quic") {
        args.use_quic = false;
    }

    if let Some(v) = matches.value_of("tpu_connection_pool_size") {
        args.tpu_connection_pool_size = v
            .parse()
            .map_err(|_| "can't parse tpu-connection-pool-size")?;
    }

    if let Some(price) = matches.value_of("compute_unit_price") {
        args.compute_unit_price = Some(
            price
                .parse()
                .map_err(|_| "can't parse compute-unit-price")?,
        );
    }

    if let Some(num) = matches.value_of("num_keypairs") {
        args.num_keypairs = num.parse().map_err(|_| "can't parse num-keypairs")?;
    }

    if let Some(v) = matches.value_of("num_lamports_per_account") {
        args.num_lamports_per_account = v
            .parse()
            .map_err(|_| "can't parse num-lamports-per-account")?;
    }

    if let Some(timeout) = matches.value_of("confirmation_timeout") {
        args.confirmation_timeout_secs = timeout
            .parse()
            .map_err(|_| "can't parse confirmation-timeout")?;
    }

    args.commitment_config = match matches.value_of("commitment_config").unwrap_or("confirmed") {
        "processed" => CommitmentConfig::processed(),
        "confirmed" => CommitmentConfig::confirmed(),
        "finalized" => CommitmentConfig::finalized(),
        _ => return Err("invalid commitment config"),
    };

    if let Some(faucet) = matches.value_of("faucet") {
        args.faucet_addr = Some(
            solana_net_utils::parse_host_port(faucet).map_err(|_| "can't parse faucet address")?,
        );
    }

    if let Some(addr) = matches.value_of("bind_address") {
        args.bind_address =
            solana_net_utils::parse_host(addr).map_err(|_| "Failed to parse bind-address")?;
    }

    if let Some(client_node_id_filename) = matches.value_of("client_node_id") {
        let client_node_id =
            read_keypair_file(client_node_id_filename).map_err(|_| "can't parse client-node-id")?;
        args.client_node_id = Some(client_node_id);
    }

    if let Some(warmup) = matches.value_of("warmup") {
        args.warmup_secs = warmup.parse().map_err(|_| "can't parse warmup")?;
    }

    Ok(args)
}

fn create_connection_cache(config: &Config) -> ConnectionCache {
    if config.use_quic {
        if let Some(client_node_id) = &config.client_node_id {
            ConnectionCache::new_with_client_options(
                "bench-tps-rate-quic",
                config.tpu_connection_pool_size,
                None, // no keypair needed for client cert
                Some((client_node_id, config.bind_address)),
                None, // stake_info
            )
        } else {
            ConnectionCache::new_quic("bench-tps-rate-quic", config.tpu_connection_pool_size)
        }
    } else {
        ConnectionCache::with_udp("bench-tps-rate-udp", config.tpu_connection_pool_size)
    }
}

fn create_client(
    config: &Config,
) -> Result<Arc<dyn TpsClient + Send + Sync>, Box<dyn std::error::Error>> {
    if config.use_tpu_client {
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            config.json_rpc_url.clone(),
            config.commitment_config,
        ));

        let connection_cache = create_connection_cache(config);

        match connection_cache {
            ConnectionCache::Udp(cache) => {
                let tpu_client = TpuClient::new_with_connection_cache(
                    rpc_client,
                    &config.websocket_url,
                    TpuClientConfig::default(),
                    cache,
                )?;
                Ok(Arc::new(tpu_client) as Arc<dyn TpsClient + Send + Sync>)
            }
            ConnectionCache::Quic(cache) => {
                let tpu_client = TpuClient::new_with_connection_cache(
                    rpc_client,
                    &config.websocket_url,
                    TpuClientConfig::default(),
                    cache,
                )?;
                Ok(Arc::new(tpu_client) as Arc<dyn TpsClient + Send + Sync>)
            }
        }
    } else {
        Ok(Arc::new(RpcClient::new_with_commitment(
            config.json_rpc_url.clone(),
            config.commitment_config,
        )))
    }
}

fn airdrop_lamports(
    client: &dyn TpsClient,
    id: &Keypair,
    amount: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let balance = client.get_balance(&id.pubkey())?;
    if balance >= amount {
        info!(
            "Authority {} already has {} SOL, skipping airdrop",
            id.pubkey(),
            Sol(balance)
        );
        return Ok(());
    }

    let request_amount = amount.saturating_sub(balance);
    info!(
        "Requesting airdrop of {} SOL to {}",
        Sol(request_amount),
        id.pubkey()
    );

    let blockhash = client.get_latest_blockhash()?;
    match client.request_airdrop_with_blockhash(&id.pubkey(), request_amount, &blockhash) {
        Ok(sig) => {
            info!("Airdrop requested: {}", sig);
            // Wait for airdrop to confirm
            thread::sleep(Duration::from_secs(2));
            let new_balance = client.get_balance(&id.pubkey())?;
            info!("New balance: {} SOL", Sol(new_balance));
            Ok(())
        }
        Err(e) => {
            // Airdrop via RPC failed, wait and check if faucet processed it anyway
            warn!("RPC airdrop request failed: {}", e);
            thread::sleep(Duration::from_secs(2));
            let new_balance = client.get_balance(&id.pubkey())?;
            if new_balance >= amount {
                info!("Balance is now sufficient: {} SOL", Sol(new_balance));
                Ok(())
            } else {
                Err(format!("Airdrop failed, balance still {} SOL", Sol(new_balance)).into())
            }
        }
    }
}

fn fund_keypairs(
    client: &dyn TpsClient,
    funding_keypair: &Keypair,
    keypairs: &[Keypair],
    lamports_per_account: u64,
    faucet_addr: Option<&SocketAddr>,
) -> Result<(), Box<dyn std::error::Error>> {
    let funding_pubkey = funding_keypair.pubkey();
    let balance = client.get_balance(&funding_pubkey)?;
    let total_needed = lamports_per_account * keypairs.len() as u64;

    info!(
        "Funding {} keypairs with {} lamports each (total needed: {} SOL)",
        keypairs.len(),
        lamports_per_account,
        Sol(total_needed)
    );
    info!("Authority {} balance: {} SOL", funding_pubkey, Sol(balance));

    // Request airdrop if needed
    if balance < total_needed {
        if faucet_addr.is_some() {
            airdrop_lamports(client, funding_keypair, total_needed)?;
        } else {
            return Err(format!(
                "Insufficient balance: have {} SOL, need {} SOL. Use --faucet to request airdrop.",
                Sol(balance),
                Sol(total_needed)
            )
            .into());
        }
    }

    // Re-check balance after potential airdrop
    let balance = client.get_balance(&funding_pubkey)?;
    if balance < total_needed {
        return Err(format!(
            "Still insufficient balance after airdrop: have {} SOL, need {} SOL",
            Sol(balance),
            Sol(total_needed)
        )
        .into());
    }

    // Fund in batches to avoid transaction size limits
    let batch_size = 20;
    for (batch_idx, chunk) in keypairs.chunks(batch_size).enumerate() {
        let blockhash = client.get_latest_blockhash()?;

        let instructions: Vec<_> = chunk
            .iter()
            .map(|kp| {
                system_instruction::transfer(&funding_pubkey, &kp.pubkey(), lamports_per_account)
            })
            .collect();

        let message = solana_sdk::message::Message::new(&instructions, Some(&funding_pubkey));
        let tx = Transaction::new(&[funding_keypair], message, blockhash);

        if let Err(e) = client.send_transaction(tx) {
            warn!("Failed to send funding batch {}: {}", batch_idx, e);
        }
    }

    // Wait for funding to settle
    info!("Waiting for funding transactions to confirm...");
    thread::sleep(Duration::from_secs(3));

    // Verify funding
    let mut funded = 0;
    for kp in keypairs {
        if let Ok(balance) = client.get_balance(&kp.pubkey()) {
            if balance >= lamports_per_account {
                funded += 1;
            }
        }
    }

    info!("Funded {}/{} keypairs", funded, keypairs.len());
    if funded < keypairs.len() / 2 {
        return Err(format!("Too few keypairs funded: {}/{}", funded, keypairs.len()).into());
    }

    Ok(())
}

// Channel capacity for pre-generated transactions
// Large enough to absorb generation latency, small enough to limit staleness
const TX_CHANNEL_CAPACITY: usize = 2000;
// Batch size for parallel generation
const TX_BATCH_SIZE: usize = 500;
// Maximum allowed lateness before dropping a transaction (in milliseconds)
// If we're more than this far behind schedule, drop the transaction to maintain timing accuracy
const MAX_SEND_LATENESS_MS: u64 = 100;

/// Spawn a producer thread that generates transactions in batches.
///
/// The producer continuously generates transactions with parallel signing
/// and sends them to the channel. It reads the current blockhash from the
/// shared state and stops when the exit signal is set.
fn spawn_producer_thread(
    thread_id: usize,
    keypairs: Vec<Keypair>,
    compute_unit_price: Option<u64>,
    tx_producer: Sender<Transaction>,
    shared_blockhash: Arc<RwLock<Hash>>,
    blockhash_generation: Arc<AtomicU64>,
    exit_signal: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(format!("producer-{}", thread_id))
        .spawn(move || {
            let mut tx_generator = TransactionGenerator::new(keypairs, compute_unit_price, false);

            let mut last_generation = 0u64;

            loop {
                if exit_signal.load(Ordering::Relaxed) {
                    break;
                }

                // Check if blockhash has changed
                let current_generation = blockhash_generation.load(Ordering::Relaxed);
                if current_generation != last_generation {
                    // Blockhash changed - old transactions in channel will be discarded
                    // by consumer, just update our tracking
                    last_generation = current_generation;
                }

                // Read current blockhash
                let blockhash = {
                    let guard = shared_blockhash.read().unwrap();
                    *guard
                };

                // Generate a batch of transactions with parallel signing
                let batch = tx_generator.generate_batch(&blockhash, TX_BATCH_SIZE);

                // Send transactions to channel (blocks if channel is full)
                for tx in batch {
                    if exit_signal.load(Ordering::Relaxed) {
                        return;
                    }
                    // send() blocks if channel is full, providing backpressure
                    if tx_producer.send(tx).is_err() {
                        // Channel closed, consumer stopped
                        return;
                    }
                }
            }
        })
        .expect("Failed to spawn producer thread")
}

/// Spawn a sender thread that sends transactions at a given rate.
///
/// Uses a producer-consumer pattern: a separate producer thread generates
/// transactions with parallel signing, while this thread consumes them
/// at the rate-limited pace. This ensures the sender never blocks on
/// transaction generation.
fn spawn_sender_thread(
    thread_id: usize,
    client: Arc<dyn TpsClient + Send + Sync>,
    keypairs: Vec<Keypair>,
    compute_unit_price: Option<u64>,
    target_tps: u64,
    exit_signal: Arc<AtomicBool>,
    global_sent_count: Arc<AtomicU64>,
    global_tracked_count: Arc<AtomicU64>,
    start_time: Instant,
    warmup_duration: Duration,
    measurement_duration: Duration,
    total_duration: Option<Duration>,
    tx_count_limit: Option<u64>,
) -> JoinHandle<u64> {
    thread::Builder::new()
        .name(format!("sender-{}", thread_id))
        .spawn(move || {
            // Get initial blockhash
            let mut last_blockhash = match client.get_latest_blockhash() {
                Ok(h) => h,
                Err(e) => {
                    error!("Thread {}: Failed to get initial blockhash: {}", thread_id, e);
                    return 0;
                }
            };

            // Shared state for producer-consumer coordination
            let shared_blockhash = Arc::new(RwLock::new(last_blockhash));
            let blockhash_generation = Arc::new(AtomicU64::new(0));

            // Create bounded channel for pre-generated transactions
            let (tx_producer, tx_consumer): (Sender<Transaction>, Receiver<Transaction>) =
                bounded(TX_CHANNEL_CAPACITY);

            // Spawn producer thread
            let producer_exit = exit_signal.clone();
            let producer_handle = spawn_producer_thread(
                thread_id,
                keypairs,
                compute_unit_price,
                tx_producer,
                shared_blockhash.clone(),
                blockhash_generation.clone(),
                producer_exit,
            );

            let mut rate_limiter = RateLimiter::new(target_tps);
            let mut local_sent = 0u64;
            let mut last_blockhash_time = Instant::now();
            let blockhash_refresh_interval = Duration::from_secs(2);
            let measurement_end = warmup_duration + measurement_duration;

            loop {
                // Check exit conditions
                if exit_signal.load(Ordering::Relaxed) {
                    break;
                }

                if let Some(dur) = total_duration {
                    if start_time.elapsed() >= dur {
                        break;
                    }
                }

                if let Some(limit) = tx_count_limit {
                    if global_tracked_count.load(Ordering::Relaxed) >= limit {
                        break;
                    }
                }

                // Refresh blockhash periodically
                if last_blockhash_time.elapsed() >= blockhash_refresh_interval {
                    match client.get_latest_blockhash() {
                        Ok(new_blockhash) => {
                            if new_blockhash != last_blockhash {
                                last_blockhash = new_blockhash;
                                last_blockhash_time = Instant::now();

                                // Update shared blockhash and increment generation
                                {
                                    let mut guard = shared_blockhash.write().unwrap();
                                    *guard = new_blockhash;
                                }
                                blockhash_generation.fetch_add(1, Ordering::Relaxed);

                                // Drain stale transactions from channel
                                let drained = drain_channel(&tx_consumer);
                                if drained > 0 {
                                    debug!(
                                        "Thread {}: Drained {} stale transactions after blockhash change",
                                        thread_id, drained
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Thread {}: Failed to refresh blockhash: {}", thread_id, e);
                        }
                    }
                }

                // Wait for next scheduled send time
                let scheduled_time = rate_limiter.wait_for_next();
                let now = Instant::now();

                // Check if we're too far behind schedule - if so, drop this slot
                let lateness = now.saturating_duration_since(scheduled_time);
                if lateness > Duration::from_millis(MAX_SEND_LATENESS_MS) {
                    // Too late - discard a transaction from channel to keep producer moving
                    let _ = tx_consumer.try_recv();
                    debug!(
                        "Thread {}: Dropped transaction due to lateness ({:?} behind schedule)",
                        thread_id, lateness
                    );
                    continue;
                }

                // Check if measurement has ended
                let elapsed = start_time.elapsed();
                let measurement_ended = elapsed >= measurement_end;

                // Get transaction from channel (blocks briefly if empty)
                let tx = match tx_consumer.recv_timeout(Duration::from_millis(100)) {
                    Ok(tx) => tx,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        // Channel temporarily empty, skip this slot
                        warn!("Thread {}: No transaction available, skipping slot", thread_id);
                        continue;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        // Producer stopped
                        break;
                    }
                };
                if let Err(e) = client.send_transaction(tx) {
                    warn!("Thread {}: Failed to send transaction: {}", thread_id, e);
                    continue;
                }

                // Track count during warmup and measurement (not after measurement ends)
                if !measurement_ended {
                    global_tracked_count.fetch_add(1, Ordering::Relaxed);
                }

                local_sent += 1;
                global_sent_count.fetch_add(1, Ordering::Relaxed);
            }

            // Signal producer to stop and wait for it
            exit_signal.store(true, Ordering::Relaxed);
            drain_channel(&tx_consumer);
            let _ = producer_handle.join();

            local_sent
        })
        .expect("Failed to spawn sender thread")
}

/// Drain all pending transactions from the channel.
/// Used when blockhash changes to discard stale transactions.
fn drain_channel(receiver: &Receiver<Transaction>) -> usize {
    let mut count = 0;
    while receiver.try_recv().is_ok() {
        count += 1;
    }
    count
}

fn run_benchmark(
    config: Config,
    client: Arc<dyn TpsClient + Send + Sync>,
) -> Result<(), Box<dyn std::error::Error>> {
    let num_threads = config.num_threads;

    info!("Starting rate-limited benchmark");
    info!("  Target TPS: {}", config.target_tps);
    info!("  Threads: {}", num_threads);
    info!(
        "  TPS per thread: {}",
        config.target_tps / num_threads as u64
    );
    if let Some(duration) = config.duration_secs {
        info!("  Duration: {}s (excluding warmup)", duration);
    }
    if let Some(tx_count) = config.tx_count {
        info!("  Transaction count: {}", tx_count);
    }
    if config.warmup_secs > 0 {
        info!("  Warmup: {}s", config.warmup_secs);
    }
    info!(
        "  Client type: {}{}",
        if config.use_tpu_client {
            "TpuClient"
        } else {
            "RpcClient"
        },
        if config.use_quic { " (QUIC)" } else { " (UDP)" }
    );
    if config.client_node_id.is_some() {
        info!("  Using staked connection");
    }

    // Generate keypairs - ensure we have at least 2 per thread
    let min_keypairs = num_threads * 2;
    let num_keypairs = config.num_keypairs.max(min_keypairs);
    info!("Generating {} keypairs...", num_keypairs);
    let keypairs: Vec<Keypair> = (0..num_keypairs).map(|_| Keypair::new()).collect();

    // Calculate minimum balance needed (rent + lamports for transfers + fees)
    let rent = client.get_minimum_balance_for_rent_exemption(0)?;
    let lamports_per_account = rent + config.num_lamports_per_account;

    // Fund keypairs
    fund_keypairs(
        client.as_ref(),
        &config.id,
        &keypairs,
        lamports_per_account,
        config.faucet_addr.as_ref(),
    )?;

    // Set up signals and global counters
    let exit_signal = Arc::new(AtomicBool::new(false));
    let measurement_start_signal = Arc::new(AtomicBool::new(false));
    let measurement_end_signal = Arc::new(AtomicBool::new(false));
    let global_sent_count = Arc::new(AtomicU64::new(0));
    let global_tracked_count = Arc::new(AtomicU64::new(0)); // Transactions sent during measurement

    // Spawn confirmation tracker (uses tx-count based confirmation like bench-tps)
    let tracker_config = ConfirmationTrackerConfig {
        poll_interval_ms: 200,
    };
    let confirmation_handle = spawn_confirmation_tracker(
        client.clone(),
        measurement_start_signal.clone(),
        measurement_end_signal.clone(),
        tracker_config,
    );

    // Split keypairs among threads
    let keypairs_per_thread = keypairs.len() / num_threads;
    let mut keypair_chunks: Vec<Vec<Keypair>> = Vec::with_capacity(num_threads);
    let mut keypair_iter = keypairs.into_iter();

    for i in 0..num_threads {
        let chunk_size = if i == num_threads - 1 {
            // Last thread gets any remaining keypairs
            keypairs_per_thread + (num_keypairs % num_threads)
        } else {
            keypairs_per_thread
        };
        let chunk: Vec<Keypair> = keypair_iter.by_ref().take(chunk_size).collect();
        keypair_chunks.push(chunk);
    }

    // Calculate TPS per thread
    let base_tps_per_thread = config.target_tps / num_threads as u64;
    let extra_tps = config.target_tps % num_threads as u64;

    // Prepare timing
    let warmup_duration = Duration::from_secs(config.warmup_secs);
    let measurement_duration = Duration::from_secs(config.duration_secs.unwrap_or(60));
    // Total duration is warmup + measurement (we stop when measurement ends)
    let total_duration = config
        .duration_secs
        .map(|_| warmup_duration + measurement_duration);
    let tx_count_limit = config.tx_count;

    let start_time = Instant::now();

    // Spawn sender threads
    info!("Starting {} sender threads...", num_threads);
    let mut sender_handles: Vec<JoinHandle<u64>> = Vec::with_capacity(num_threads);

    for (thread_id, thread_keypairs) in keypair_chunks.into_iter().enumerate() {
        // Distribute TPS: first `extra_tps` threads get 1 extra TPS
        let thread_tps = if (thread_id as u64) < extra_tps {
            base_tps_per_thread + 1
        } else {
            base_tps_per_thread
        };

        let handle = spawn_sender_thread(
            thread_id,
            client.clone(),
            thread_keypairs,
            config.compute_unit_price,
            thread_tps,
            exit_signal.clone(),
            global_sent_count.clone(),
            global_tracked_count.clone(),
            start_time,
            warmup_duration,
            measurement_duration,
            total_duration,
            tx_count_limit,
        );
        sender_handles.push(handle);
    }

    // Progress reporting loop
    let progress_interval = Duration::from_secs(1);
    let mut last_progress_time = Instant::now();
    let mut warmup_logged = config.warmup_secs == 0;
    let mut measurement_end_logged = false;
    let measurement_end_time = warmup_duration + measurement_duration;
    // Track the count at measurement start/end to report accurate statistics
    let mut tracked_at_measurement_start: Option<u64> = None;
    let mut tracked_at_measurement_end: Option<u64> = None;
    let mut last_tracked = 0;

    if config.warmup_secs > 0 {
        info!("Warmup phase started...");
    } else {
        info!("Measurement phase started...");
        tracked_at_measurement_start = Some(global_tracked_count.load(Ordering::Relaxed));
    }

    loop {
        thread::sleep(Duration::from_millis(100));

        // Check if all sender threads are done
        let all_done = sender_handles.iter().all(|h| h.is_finished());
        if all_done {
            break;
        }

        let elapsed = start_time.elapsed();
        let in_warmup = elapsed < warmup_duration;
        let measurement_ended = elapsed >= measurement_end_time;
        let in_measurement = !in_warmup && !measurement_ended;

        // Log transition from warmup to measurement and signal confirmation tracker
        if !in_warmup && !warmup_logged {
            info!("Warmup complete. Measurement phase started...");
            last_progress_time = Instant::now();
            measurement_start_signal.store(true, Ordering::Relaxed);
            tracked_at_measurement_start = Some(global_tracked_count.load(Ordering::Relaxed));
            warmup_logged = true;
        }

        // Signal everything to stop when measurement ends
        if measurement_ended && !measurement_end_logged {
            // Capture tracked count before signaling measurement end
            tracked_at_measurement_end = Some(global_tracked_count.load(Ordering::Relaxed));
            info!("Measurement complete. Stopping benchmark...");
            measurement_end_signal.store(true, Ordering::Relaxed);
            // Signal sender threads to stop
            exit_signal.store(true, Ordering::Relaxed);
            measurement_end_logged = true;
        }

        // Print progress
        if last_progress_time.elapsed() >= progress_interval {
            let sent = global_sent_count.load(Ordering::Relaxed);
            let tracked = global_tracked_count.load(Ordering::Relaxed);
            let elapsed_secs = elapsed.as_secs_f64();
            let actual_tps = sent as f64 / elapsed_secs;

            if in_warmup {
                info!(
                    "[WARMUP] {} sent ({:.1} TPS, {:.1}s elapsed)",
                    sent, actual_tps, elapsed_secs
                );
            } else if in_measurement {
                last_tracked =
                    std::cmp::max(last_tracked, tracked_at_measurement_start.unwrap_or(0));
                let measurement_elapsed = last_progress_time.elapsed().as_millis() as f64;
                // Calculate delta from measurement start for accurate TPS reporting
                let measurement_tracked = tracked.saturating_sub(last_tracked);
                last_tracked = tracked;
                let tracked_tps = if measurement_elapsed > 0.0 {
                    (measurement_tracked * 1000) as f64 / measurement_elapsed
                } else {
                    0.0
                };
                info!(
                    "[MEASURE] {} tracked ({:.1} TPS, {:.2}s measurement time)",
                    measurement_tracked,
                    tracked_tps,
                    elapsed.saturating_sub(warmup_duration).as_secs_f64()
                );
            }
            // No cooldown logging - sender threads exit when measurement ends
            last_progress_time = Instant::now();
        }
    }

    // Ensure measurement end is signaled and tracked count is captured (in case we finished early)
    if !measurement_end_logged {
        tracked_at_measurement_end = Some(global_tracked_count.load(Ordering::Relaxed));
        measurement_end_signal.store(true, Ordering::Relaxed);
    }

    // Wait for all sender threads
    let mut total_sent = 0u64;
    for handle in sender_handles {
        match handle.join() {
            Ok(sent) => total_sent += sent,
            Err(_) => warn!("A sender thread panicked"),
        }
    }

    let total_elapsed = start_time.elapsed();
    let tracked_count = global_tracked_count.load(Ordering::Relaxed);

    info!(
        "Send loop completed: {} total sent ({} tracked for confirmation) in {:.1}s",
        total_sent,
        tracked_count,
        total_elapsed.as_secs_f64()
    );

    // Wait for confirmation tracker (it already stopped when measurement ended)
    info!("Waiting for confirmation tracker to finish...");
    let result = confirmation_handle
        .join()
        .expect("Confirmation tracker panicked");

    // Calculate transactions sent during measurement from our tracked counts
    let measurement_sent = tracked_at_measurement_end
        .unwrap_or(0)
        .saturating_sub(tracked_at_measurement_start.unwrap_or(0));

    // Generate and print report using measurement duration
    // Note: sent and confirmed are independent measurements under the steady-state model
    // - sent = transactions sent during measurement window
    // - confirmed = confirmations observed during measurement window (includes some warmup TXs)
    let report =
        solana_bench_tps_rate::stats::StatsReport::new(measurement_sent as usize, result.confirmed);
    report.print(measurement_duration.as_secs_f64(), config.target_tps);

    Ok(())
}

fn main() {
    solana_logger::setup_with_default_filter();
    solana_metrics::set_panic_hook("bench-tps-rate", None);

    let matches = build_args(solana_version::version!()).get_matches();
    let config = match parse_args(&matches) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Error: {}", error);
            exit(1);
        }
    };

    info!("Connecting to {} ...", config.json_rpc_url);

    let client = match create_client(&config) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("Could not create client: {:?}", error);
            exit(1);
        }
    };

    // Verify connection
    match client.get_epoch_info() {
        Ok(epoch_info) => info!("Connected. Current slot: {}", epoch_info.absolute_slot),
        Err(e) => {
            eprintln!("Failed to connect to cluster: {:?}", e);
            exit(1);
        }
    }

    if let Err(e) = run_benchmark(config, client) {
        eprintln!("Benchmark failed: {:?}", e);
        exit(1);
    }
}
