//! `phonelink` — the headless CLI over the engine, with file-backed stand-ins for the OS edges.
//! `decode`, `status`, `store-keys` and `sign-out` are offline. `sign-in`, `enroll` and `session` contact Microsoft's
//! production services and wake the phone.

mod decode;
mod notifier;
mod store;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use phonelink_engine::api::events::{EngineEvent, EventBridge, EventSink};
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::standins::{FixedLock, FixedReachability};
use phonelink_engine::edge::{Edges, HostInfo, LockState, SystemClock};
use phonelink_engine::engine::standins::{FixedPcStatus, PcState, RefusingPowerControl};
use phonelink_engine::engine::{EngineCore, FeatureEdges, FetchMode, StoreLocation, sign_out_stored};
use phonelink_engine::tap::{JsonlTap, Tap, null_tap};

#[derive(Parser)]
#[command(name = "phonelink", version, about = "Headless driver of the Phone Link macOS port engine")]
struct Cli {
    /// State directory for the file-backed store (default: <repository root>/.local/port, gitignored).
    #[arg(long, global = true, env = "PHONELINK_STATE_DIR")]
    state_dir: Option<PathBuf>,
    /// Directory of the features' stores — notifications, messages, contacts, photos (default:
    /// <repository root>/.local/port-data, gitignored).
    #[arg(long, global = true, env = "PHONELINK_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Test seam: base URL of every DCG environment (default: production hosts).
    #[arg(long, global = true, hide = true)]
    dcg_base: Option<String>,
    /// Test seam: base URL of the relay hub.
    #[arg(long, global = true, hide = true)]
    hub_base: Option<String>,
    /// Test seam: the PC identities, `pl+wea` (the app's: Phone Link's beside the Cross Device
    /// Experience Host's), or one identity carrying every feature, `wea` or `pl`.
    #[arg(long, global = true, hide = true, default_value = "pl+wea")]
    profile: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode captured bytes at one seam and print JSON (offline).
    Decode {
        #[arg(value_enum)]
        layer: decode::Layer,
        /// Hex bytes (whitespace ignored), `@file` for a binary file, or base64 with --base64.
        input: String,
        #[arg(long)]
        base64: bool,
    },
    /// Show the persisted enrollment and trust state (offline).
    Status,
    /// List the keys in the state directory (offline; values are never printed).
    StoreKeys,
    /// Sign out, locally (offline): both identities forget the Microsoft account and their state,
    /// and the selected phone and every phone's feature stores are deleted.
    SignOut,
    /// LIVE: MSA sign-in. Needs a sign-in already stored in the state directory (the engine's
    /// interactive sign-in needs the app's `WebView`); fails loudly otherwise.
    SignIn,
    /// LIVE: sign-in, enrollment of the app's identity, and trust synchronization.
    Enroll,
    /// LIVE: deletes this state directory's own identity from the Microsoft account
    /// (`DeleteDeviceInfo`) — and, with `--profile pl+wea`, the Cross Device identity's too; the
    /// state directory is not usable afterwards.
    DeleteOwnDeviceInfo,
    /// LIVE: the full start sequence — relay, wake, `/SessionValidation`, `connect` — then keep the
    /// phone session up until Ctrl-C (or `--seconds`).
    Session {
        /// The phone's dcgClientId when the account links several.
        #[arg(long)]
        phone: Option<String>,
        /// Directory for the decode-seam tap (JSON lines, unredacted except bearer tokens): keep it out
        /// of version control.
        #[arg(long)]
        tap: Option<PathBuf>,
        /// Stop after this many seconds.
        #[arg(long)]
        seconds: Option<u64>,
    },
}

fn default_state_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.local/port"))
}

fn default_data_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.local/port-data"))
}

/// The host name for `metadata.displayName` (the engine cuts it before its first `.`).
struct ProcessHost;

impl HostInfo for ProcessHost {
    fn host_name(&self) -> String {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "phonelink-cli".into())
    }

    /// The region of the POSIX locale (`LC_ALL`, else `LANG`: `ko_KR.UTF-8` → `KR`).
    fn region_code(&self) -> Option<String> {
        ["LC_ALL", "LANG"].iter().find_map(|var| {
            let value = std::env::var(var).ok()?;
            let region = value.split('.').next()?.split('_').nth(1)?;
            (region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic())).then(|| region.to_ascii_uppercase())
        })
    }
}

/// Prints the sign-in code a user must act on, and every other event as one line.
struct StderrEvents;

impl EventSink for StderrEvents {
    fn on_event(&self, event: EngineEvent) {
        match event {
            EngineEvent::WebSignInRequested { request_id, request } => {
                eprintln!("web sign-in requested ({request_id}): {} — needs the app's web view", request.url);
            }
            other => eprintln!("[event] {other:?}"),
        }
    }
}

fn config(cli: &Cli) -> Result<EngineConfig, String> {
    let mut c = match cli.profile.as_str() {
        "pl+wea" => EngineConfig::production(),
        "wea" => EngineConfig::default(),
        "pl" => {
            let mut c = EngineConfig::default();
            c.identity.profile = phonelink_engine::constants::PROFILE_PL;
            c
        }
        other => return Err(format!("--profile must be pl+wea, wea or pl, not {other:?}")),
    };

    if let Some(base) = &cli.dcg_base {
        c.endpoints.prod_base = base.clone();
        c.endpoints.beta_base = base.clone();
        c.endpoints.dogfood_base = base.clone();
        c.endpoints.discovery_base = base.clone();
    }
    c.endpoints.hub_base = cli.hub_base.clone();
    Ok(c)
}

fn edges(cli: &Cli) -> Result<Edges, String> {
    let dir = cli.state_dir.clone().unwrap_or_else(default_state_dir);
    let store = store::FileStore::open(&dir).map_err(|e| e.to_string())?;

    // CLI stand-ins: the session counts as unlocked and the network as reachable.
    Ok(Edges {
        store: Arc::new(store),
        lock: Arc::new(FixedLock::new(LockState::Unlocked)),
        host: Arc::new(ProcessHost),
        reachability: Arc::new(FixedReachability::online()),
        clock: Arc::new(SystemClock),
        // The CLI has no web view: an interactive sign-in refuses (a session must already be stored).
        web_sign_in: Arc::new(phonelink_engine::edge::standins::RefusingWebSignIn),
        token_transport: None,
    })
}

fn feature_stores(cli: &Cli) -> StoreLocation {
    StoreLocation::Directory(cli.data_dir.clone().unwrap_or_else(default_data_dir))
}

fn engine(cli: &Cli, tap: Tap) -> Result<Arc<EngineCore>, String> {
    let edges = edges(cli)?;

    // CLI stand-ins of the features' OS edges: notifications are printed; there is no pasteboard,
    // image codec, attachment imaging, desktop picture or Bluetooth of the host (so `/wallpaper` is
    // answered as a desktop without a picture), and the PC's power and network state cannot be read —
    // so the phone's `/remotedevicestatus` is answered `UNKNOWN_ERROR` rather than with invented
    // values.
    let mut features = FeatureEdges::stand_ins(feature_stores(cli), Arc::new(notifier::StderrNotifier));
    features.pc_status = FixedPcStatus::new(PcState { failing: true, ..PcState::default() });

    // The CLI never locks, sleeps, shuts down or restarts its host for the phone.
    features.pc_power = Arc::new(RefusingPowerControl);
    EngineCore::build(config(cli)?, edges, features, tap, EventBridge::new(Arc::new(StderrEvents)))
        .map_err(|e| e.to_string())
}

fn tap_into(dir: &PathBuf) -> Result<Tap, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("tap-{secs}.jsonl"));
    let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("tap: {}", path.display());
    Ok(Arc::new(JsonlTap::new(Box::new(std::io::BufWriter::new(file)))))
}

async fn run(cli: Cli) -> Result<(), String> {
    match &cli.command {
        Command::Decode { layer, input, base64 } => {
            let json = decode::read_input(input, *base64).and_then(|b| decode::decode(*layer, &b))?;
            println!("{}", serde_json::to_string_pretty(&json).expect("JSON"));
        }
        Command::StoreKeys => {
            let dir = cli.state_dir.clone().unwrap_or_else(default_state_dir);
            let s = store::FileStore::open(&dir).map_err(|e| e.to_string())?;
            eprintln!("# {}", s.root().display());
            for k in s.keys() {
                println!("{k}");
            }
        }
        Command::Status => print_status(&cli)?,
        Command::SignOut => {
            sign_out_stored(&config(&cli)?, &edges(&cli)?, &feature_stores(&cli)).map_err(|e| e.to_string())?;
            println!("signed out");
        }
        Command::SignIn => {
            let core = engine(&cli, null_tap())?;
            core.auth.sign_in().await.map_err(|e| e.to_string())?;
            println!("signed in");
        }
        Command::Enroll => enroll(&cli).await?,
        Command::DeleteOwnDeviceInfo => delete_own_device_info(&cli).await?,
        Command::Session { phone, tap, seconds } => run_session(&cli, phone.as_deref(), tap.as_ref(), *seconds).await?,
    }
    Ok(())
}

/// `status`: the account, the trust relationship count and the peers.
fn print_status(cli: &Cli) -> Result<(), String> {
    let core = engine(cli, null_tap())?;
    match core.auth.account_info() {
        None => println!("account: not provisioned"),
        Some(a) => println!(
            "account: {:?}, environment {:?}, dcgClientId {}",
            a.state,
            a.environment,
            a.dcg_client_id.as_deref().unwrap_or("-")
        ),
    }

    println!("trust relationships: {}", core.auth.trust_relationships().len());
    for p in core.auth.peers() {
        println!(
            "peer {} type {} os {} version {}",
            p.client,
            p.info.client_type.as_deref().unwrap_or("-"),
            p.info.os_name.as_deref().unwrap_or("-"),
            p.info.client_version.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

/// `enroll`: sign in, provision, and list the peers of a forced trust sync.
async fn enroll(cli: &Cli) -> Result<(), String> {
    let core = engine(cli, null_tap())?;
    core.auth.sign_in().await.map_err(|e| e.to_string())?;
    let p = core.auth.ensure_provisioned().await.map_err(|e| e.to_string())?;
    println!("enrolled as {} ({:?})", p.own, p.env);

    for peer in core.auth.sync_trust(FetchMode::ForceCloud).await.map_err(|e| e.to_string())? {
        println!(
            "peer {} type {} os {} distinguishedDeviceId {}",
            peer.client,
            peer.info.client_type.as_deref().unwrap_or("-"),
            peer.info.os_name.as_deref().unwrap_or("-"),
            peer.info.distinguished_device_id.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

/// `delete-own-device-info`: removes each enrolled stack's own device-list entry from the account.
async fn delete_own_device_info(cli: &Cli) -> Result<(), String> {
    let core = engine(cli, null_tap())?;
    let mut stacks = vec![("primary", core.auth.clone())];
    if let Some(cdeh) = &core.cdeh {
        stacks.push(("cdeh", cdeh.auth.clone()));
    }

    for (name, auth) in stacks {
        let Some(own) = auth.own_client() else {
            println!("{name}: no enrolled identity");
            continue;
        };
        match auth.delete_own_device_info().await {
            Ok(()) => println!("{name}: deleted {own} from the account"),
            Err(e) => println!("{name}: {own} not deleted: {e}"),
        }
    }
    Ok(())
}

/// `session`: runs the engine with the phone until the time is up or Ctrl-C.
async fn run_session(
    cli: &Cli,
    phone: Option<&str>,
    tap: Option<&PathBuf>,
    seconds: Option<u64>,
) -> Result<(), String> {
    let tap = match tap {
        Some(dir) => tap_into(dir)?,
        None => null_tap(),
    };

    let core = engine(cli, tap)?;
    let started = core.start(phone).await.map_err(|e| e.to_string())?;
    let Some(phone) = &started.phone else {
        return Err("no phone selected: pass --phone <dcgClientId> once".into());
    };
    eprintln!("own {}; phone {}", started.own, phone.client);

    match seconds {
        Some(s) => tokio::time::sleep(Duration::from_secs(s)).await,
        None => {
            tokio::signal::ctrl_c().await.map_err(|e| e.to_string())?;
        }
    }

    core.stop().await;
    eprintln!("stopped");
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("phonelink: {e}");
            ExitCode::FAILURE
        }
    }
}
