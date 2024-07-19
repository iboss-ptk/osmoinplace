#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))]

use std::{
    io::{Seek, Write},
    path::PathBuf,
    process::Command,
    time::Duration,
};

use clap::{Parser, Subcommand};
use color_eyre::eyre::{eyre, Context, Ok, Result};
use colored::Colorize;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest;
use tempfile;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// osmosis home directory, defaulted to ~/.osmosisd
    #[arg(long)]
    home_dir: Option<PathBuf>,

    /// osmosis binary
    #[arg(long, default_value = "osmosisd")]
    osmosisd_bin: PathBuf,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Download mainnet state
    DownloadMainnetState,

    /// Backup current osmosis state
    Backup {
        /// Path to backup directory, defaults to $HOME/.osmosisd_bak
        #[arg(short, long)]
        path: Option<PathBuf>,
    },

    /// Restore osmosis state from a backup
    Restore {
        /// Path to backup directory, defaults to $HOME/.osmosisd_bak
        #[arg(short, long)]
        path: Option<PathBuf>,
    },

    /// Start the node and sync to the latest block
    StartSync {
        /// Stop the node on first indexed block events
        #[arg(short, long)]
        stop_on_first_indexed_block_events: bool,
    },

    /// Start osmosis in place testnet
    StartInPlaceTestnet {
        /// Optional upgrade handler, if set, the chain will be marked to run the upgrade handler when running with the right binary
        #[arg(long)]
        upgrade_handler: Option<String>,

        /// New osmosisd binary to use to run the upgrade
        #[arg(long)]
        new_osmosisd_bin: Option<PathBuf>,

        /// Command to run on first indexed block events
        #[arg(long)]
        on_ready: Option<String>,
    },

    /// Start a standalone node
    StartStandalone {
        /// Command to run on first indexed block events
        #[arg(long)]
        on_ready: Option<String>,
    },

    /// Magic start command to perform all setup at once
    MagicStart {
        /// Determine whether to download new snapshot or restore from backup
        #[arg(long, default_value = "false")]
        download_mainnet_state: bool,

        /// Path to backup directory, defaults to $HOME/.osmosisd_bak
        #[arg(long)]
        backup_path: Option<PathBuf>,

        /// Optional upgrade handler, if set, the chain will be marked to run the upgrade handler when running with the right binary
        #[arg(long)]
        upgrade_handler: Option<String>,

        /// New osmosisd binary to use to run the upgrade
        #[arg(long)]
        new_osmosisd_bin: Option<PathBuf>,

        /// Command to run on first indexed block events
        #[arg(long)]
        on_ready: Option<String>,
    },
}

const LATEST_SNAPSHOT_FETCH_URL: &str = "https://snapshots.osmosis.zone/latest";

const GENESIS_URL: &str =
    "https://github.com/osmosis-labs/osmosis/raw/main/networks/osmosis-1/genesis.json";

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    run_cmd(Cli::parse()).await
}

async fn run_cmd(cli: Cli) -> Result<()> {
    // Check if osmosisd exists
    let osmosisd = cli.osmosisd_bin;
    if which::which(osmosisd.as_os_str()).is_err() {
        return Err(eyre!("osmosisd not found in PATH"));
    }

    let osmosis_home = cli
        .home_dir
        .unwrap_or_else(|| PathBuf::from(format!("{}/.osmosisd", std::env::var("HOME").unwrap())));

    match &cli.command {
        Commands::DownloadMainnetState => download_mainnet_state(&osmosisd, &osmosis_home).await?,
        Commands::Backup { path } => backup(&osmosis_home, path.clone()).await?,
        Commands::Restore { path } => restore(&osmosis_home, path.clone()).await?,
        Commands::StartSync {
            stop_on_first_indexed_block_events,
        } => {
            start_sync(
                &osmosisd,
                &osmosis_home,
                *stop_on_first_indexed_block_events,
            )
            .await?
        }
        Commands::StartInPlaceTestnet {
            upgrade_handler,
            new_osmosisd_bin,
            on_ready,
        } => {
            start_in_place_testnet(
                &osmosisd,
                &osmosis_home,
                upgrade_handler,
                new_osmosisd_bin,
                on_ready.clone(),
            )
            .await?
        }
        Commands::StartStandalone { on_ready } => {
            start_standalone(&osmosisd, &osmosis_home, on_ready.clone())?
        }
        Commands::MagicStart {
            download_mainnet_state: download,
            backup_path,
            upgrade_handler,
            new_osmosisd_bin,
            on_ready,
        } => {
            if *download {
                download_mainnet_state(&osmosisd, &osmosis_home).await?;
            } else {
                restore(&osmosis_home, backup_path.clone()).await?;
            }

            // sync the chain to first block after snapshot
            start_sync(&osmosisd, &osmosis_home, true).await?;

            // start the node
            start_in_place_testnet(
                &osmosisd,
                &osmosis_home,
                upgrade_handler,
                new_osmosisd_bin,
                on_ready.clone(),
            )
            .await?;
        }
    }

    Ok(())
}

async fn download_mainnet_state(osmosisd: &PathBuf, osmosis_home: &PathBuf) -> Result<()> {
    // Remove existing OSMOSIS_HOME directory if it exists
    if std::path::Path::new(&osmosis_home).exists() {
        spinner! {
            "Removing existing OSMOSIS_HOME directory...",
            "✓ Removed existing OSMOSIS_HOME directory.",
            std::fs::remove_dir_all(&osmosis_home).wrap_err(format!(
                "Failed to remove existing OSMOSIS_HOME directory: {}",
                osmosis_home.display()
            ))?
        };
    }

    // Initialize osmosisd
    spinner! {
        "Initializing osmosis chain...",
        "✓ Initialized osmosis chain.",
        Command::new(osmosisd)
            .arg("init")
            .arg("test")
            .arg("--chain-id")
            .arg("edgenet")
            .arg("--home")
            .arg(&osmosis_home)
            .stderr(std::process::Stdio::null())
            .status()
            .wrap_err("Failed to initialize osmosis chain")?
    };

    // Download genesis file
    spinner! {
        "Downloading genesis file...",
        "✓ Downloaded genesis file.",
        {
            let genesis_content = reqwest::get(GENESIS_URL)
                .await?
                .text()
                .await
                .wrap_err("Failed to download genesis file")?;

            std::fs::write(osmosis_home.join("config").join("genesis.json"), genesis_content)
                .wrap_err("Failed to write genesis file")?;
        }
    };

    // Get snapshot URL
    let snapshot_url = spinner! {
        "Downloading latest snapshot...",
        "✓ Fetched latest snapshot url.",
        reqwest::get(LATEST_SNAPSHOT_FETCH_URL)
            .await?
            .text()
        .await?
    };

    // Download latest snapshot
    let snapshot_response = reqwest::get(snapshot_url.trim())
        .await
        .wrap_err("Failed to fetch snapshot")?;
    let total_size = snapshot_response
        .content_length()
        .ok_or_else(|| eyre!("Failed to get snapshot size from response"))?;

    // Indicatif setup
    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
                .template("{msg}\n{spinner:.cyan} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")?
                .progress_chars("#>-"));
    pb.set_message("Downloading latest snapshot...".cyan().to_string());

    let mut downloaded_bytes: u64 = 0;
    let mut stream = snapshot_response.bytes_stream();

    // Create a temporary file to store the downloaded snapshot
    let mut temp_file = tempfile::tempfile().wrap_err("Failed to create temporary file")?;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.wrap_err("Failed to download chunk")?;
        downloaded_bytes += chunk.len() as u64;
        temp_file
            .write_all(&chunk)
            .wrap_err("Failed to write chunk to temporary file")?;
        pb.set_position(downloaded_bytes);
    }

    pb.finish_with_message("✓ Downloaded latest snapshot.".green().to_string());

    // Decompress snapshot using lz4 and extract using tar
    spinner! {
        "Decompressing and extracting snapshot...",
        "✓ Decompressed and extracted snapshot.",
        {
            temp_file.seek(std::io::SeekFrom::Start(0)).wrap_err("Failed to seek to start of temporary file")?;
            let mut decoder = lz4::Decoder::new(temp_file).wrap_err("Failed to create lz4 decoder")?;
            let mut archive = tar::Archive::new(&mut decoder);
            archive.unpack(&osmosis_home).wrap_err("Failed to extract snapshot")
        }
    }?;

    Ok(())
}

async fn backup(osmosis_home: &PathBuf, path: Option<PathBuf>) -> Result<()> {
    let backup_path = path.unwrap_or_else(|| {
        PathBuf::from(format!("{}/.osmosisd_bak", std::env::var("HOME").unwrap()))
    });

    // Cleanup if backup path already exists
    if backup_path.exists() {
        spinner! {
            "Removing existing backup directory...",
            "✓ Removed existing backup directory.",
            {
                std::fs::remove_dir_all(&backup_path)
                    .wrap_err("Failed to remove existing backup directory")
            }
        }?;
    }

    // Copy home to backup
    spinner! {
        &format!("Copying {} to {}...", osmosis_home.display(), backup_path.display()),
        &format!("✓ Copied {} to {}.", osmosis_home.display(), backup_path.display()),
        {
            let osmosis_home_path = PathBuf::from(osmosis_home.clone());
            let options = fs_extra::dir::CopyOptions::new().copy_inside(true);

            fs_extra::dir::copy(&osmosis_home_path, &backup_path, &options).wrap_err("Failed to copy home to backup")
        }
    }?;

    Ok(())
}

async fn restore(osmosis_home: &PathBuf, path: Option<PathBuf>) -> Result<()> {
    let backup_path = path.unwrap_or_else(|| {
        PathBuf::from(format!("{}/.osmosisd_bak", std::env::var("HOME").unwrap()))
    });

    // Cleanup if osmosis home already exists
    if osmosis_home.exists() {
        spinner! {
            "Removing existing osmosis home directory...",
            "✓ Removed existing osmosis home directory.",
            std::fs::remove_dir_all(&osmosis_home).wrap_err("Failed to remove existing osmosis home directory")
        }?;
    }

    // Copy backup to home
    spinner! {
        &format!("Copying {} to {}...", backup_path.display(), osmosis_home.display()),
        &format!("✓ Copied {} to {}.", backup_path.display(), osmosis_home.display()),
        {
            let options = fs_extra::dir::CopyOptions::new().copy_inside(true);
            fs_extra::dir::copy(&backup_path, &osmosis_home, &options).wrap_err("Failed to copy backup to home")
        }
    }?;

    Ok(())
}

async fn start_sync(
    osmosisd: &PathBuf,
    osmosis_home: &PathBuf,
    stop_on_first_indexed_block_events: bool,
) -> Result<()> {
    // Start osmosisd
    let mut child = Command::new(osmosisd)
        .arg("start")
        .arg("--home")
        .arg(&osmosis_home)
        .stdout(std::process::Stdio::piped())
        .spawn()?;

    if let Some(stdout) = child.stdout.as_mut() {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let line = line?;
            println!("{}", line);
            if stop_on_first_indexed_block_events && line.contains("indexed block events") {
                child.kill()?;
                break;
            }
        }
    }

    child.wait()?;

    Ok(())
}

async fn start_in_place_testnet(
    osmosisd: &PathBuf,
    osmosis_home: &PathBuf,
    upgrade_handler: &Option<String>,
    new_osmosisd_bin: &Option<PathBuf>,
    on_ready: Option<String>,
) -> Result<()> {
    let mut cmd = Command::new(osmosisd);
    cmd.arg("in-place-testnet")
        .arg("edgenet")
        .arg("osmo12smx2wdlyttvyzvzg54y2vnqwq2qjateuf7thj")
        .arg("--home")
        .arg(&osmosis_home)
        .stdout(std::process::Stdio::piped());

    // trigger testnet upgrade if upgrade handler is set
    if let Some(upgrade_handler) = upgrade_handler {
        cmd.arg("--trigger-testnet-upgrade").arg(upgrade_handler);
    }

    let mut child = cmd.spawn()?;

    let mut on_ready_executed = false;

    if let Some(stdout) = child.stdout.as_mut() {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let line = line?;
            println!("{}", line);

            if let Some(ref on_ready) = on_ready {
                // on_ready only execute here if there is no upgrade_handler, if there is, it will be executed in `start_standalone`
                if upgrade_handler.is_none() && !on_ready_executed {
                    let status = Command::new("sh").arg("-c").arg(on_ready).spawn()?.wait()?;

                    if !status.success() {
                        return Err(eyre!("Failed to execute on_ready command"));
                    }

                    on_ready_executed = true;
                }
            }

            if line.contains("CONSENSUS FAILURE!!!") {
                child.kill()?;
                break;
            }
        }
    }

    child.wait()?;

    if let Some(new_osmosisd_bin) = new_osmosisd_bin {
        start_standalone(new_osmosisd_bin, osmosis_home, on_ready)?;
    }

    Ok(())
}

fn start_standalone(
    osmosisd: &PathBuf,
    osmosis_home: &PathBuf,
    on_ready: Option<String>,
) -> Result<()> {
    let mut child = start_node_no_peers(&mut Command::new(osmosisd), &osmosis_home)
        .stdout(std::process::Stdio::piped())
        .spawn()?;

    let mut on_ready_executed = false;

    if let Some(stdout) = child.stdout.as_mut() {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let line = line?;
            println!("{}", line);
            if let Some(ref on_ready) = on_ready {
                if !on_ready_executed && line.contains("indexed block events") {
                    let status = Command::new("sh").arg("-c").arg(on_ready).spawn()?.wait()?;

                    if !status.success() {
                        return Err(eyre!("Failed to execute on_ready command"));
                    }

                    on_ready_executed = true;
                }
            }
        }
    }

    child.wait()?;

    Ok(())
}

fn start_node_no_peers<'a>(
    osmosisd: &'a mut Command,
    osmosis_home: &'a PathBuf,
) -> &'a mut Command {
    osmosisd
        .arg("start")
        .arg("--home")
        .arg(&osmosis_home)
        .arg("--p2p.persistent_peers")
        .arg("")
        .arg("--p2p.seeds")
        .arg("")
        .arg("--rpc.unsafe")
        .arg("--grpc.enable")
        .arg("--grpc-web.enable")
}

#[macro_export]
macro_rules! spinner {
    ($message:expr, $finished_message:expr, $e:expr) => {{
        let spinner = ProgressBar::new_spinner();
        spinner.set_message($message.cyan().to_string());
        spinner.enable_steady_tick(Duration::from_millis(100));

        let result = $e;

        spinner.finish_with_message($finished_message.green().to_string());
        result
    }};
}
