use clap::Parser;
use ipc_channel::ipc;
use memtrack::prelude::*;
use memtrack::{MemtrackIpcMessage, Tracker, handle_ipc_message};
use runner_shared::artifacts::{ArtifactExt, MemtrackArtifact, MemtrackEvent, MemtrackWriter};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "memtrack")]
#[command(version, about = "Track memory allocations using eBPF", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser)]
enum Commands {
    /// Track memory allocations for a command
    Track {
        /// Command to execute and track
        command: String,

        /// Output folder for the allocations data
        #[arg(short, long, default_value = ".")]
        output: PathBuf,

        /// Optional IPC server name for receiving control commands
        #[arg(long)]
        ipc_server: Option<String>,
    },
}

/// Get the original user's UID and GID when running under sudo.
/// Returns None if not running under sudo or if the environment variables are not set.
fn get_user_uid_gid() -> Option<(u32, u32)> {
    let uid = std::env::var("SUDO_UID").ok()?.parse().ok()?;
    let gid = std::env::var("SUDO_GID").ok()?.parse().ok()?;
    Some((uid, gid))
}

fn main() -> Result<()> {
    env_logger::builder()
        .parse_env(env_logger::Env::new().filter_or("CODSPEED_LOG", "info"))
        .format_timestamp(None)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Track {
            command,
            output: out_dir,
            ipc_server,
        } => {
            debug!("Starting memtrack for command: {command}");

            let status =
                track_command(&command, ipc_server, &out_dir).context("Failed to track command")?;

            std::process::exit(status.code().unwrap_or(1));
        }
    }
}

fn track_command(
    cmd_string: &str,
    ipc_server_name: Option<String>,
    out_dir: &Path,
) -> anyhow::Result<std::process::ExitStatus> {
    // First, establish IPC connection if needed to avoid timeouts on the runner because
    // creating the Tracker instance takes some time.
    let ipc_channel = if let Some(server_name) = ipc_server_name {
        debug!("Connecting to IPC server: {server_name}");

        let (tx, rx) = ipc::channel::<MemtrackIpcMessage>()?;
        let sender = ipc::IpcSender::connect(server_name)?;
        sender.send(tx)?;

        Some(rx)
    } else {
        None
    };

    let tracker = Tracker::new()?;
    let tracker_arc = Arc::new(Mutex::new(tracker));

    // Spawn IPC handler thread with the now-available tracker
    let ipc_handle = if let Some(rx) = ipc_channel {
        let tracker_clone = tracker_arc.clone();
        Some(thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                handle_ipc_message(msg, &tracker_clone);
            }
        }))
    } else {
        None
    };

    // Start the target command using bash to handle shell syntax
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(cmd_string);

    // Drop privileges if running under sudo. This is required to avoid permission issues
    // when the target command tries to access files or directories that the current user
    // does not have permission to access.
    if let Some((uid, gid)) = get_user_uid_gid() {
        debug!("Running under sudo, dropping privileges to uid={uid}, gid={gid}");
        cmd.uid(uid).gid(gid);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn child process: {e}"))?;
    let root_pid = child.id() as i32;
    let event_rx = { tracker_arc.lock().unwrap().track(root_pid)? };
    debug!("Spawned child with pid {root_pid}");

    // Generate output file name and create file for streaming events
    let file_name = MemtrackArtifact::file_name(Some(root_pid));
    let out_file = std::fs::File::create(out_dir.join(file_name))?;

    let (write_tx, write_rx) = channel::<MemtrackEvent>();

    // Stage A: Fast drain thread - This is required so that we immediately clear the ring buffer
    // because it only has a limited size.
    static DRAIN_EVENTS: AtomicBool = AtomicBool::new(true);
    let write_tx_clone = write_tx.clone();
    let drain_thread = thread::spawn(move || {
        // Regular draining loop
        while DRAIN_EVENTS.load(Ordering::Relaxed) {
            let Ok(event) = event_rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            let _ = write_tx_clone.send(event);
        }

        // Final aggressive drain - keep trying until truly empty
        loop {
            match event_rx.try_recv() {
                Ok(event) => {
                    let _ = write_tx_clone.send(event);
                }
                Err(_) => {
                    // Sleep briefly and try once more to catch late arrivals
                    thread::sleep(Duration::from_millis(50));
                    if let Ok(event) = event_rx.try_recv() {
                        let _ = write_tx_clone.send(event);
                    } else {
                        break;
                    }
                }
            }
        }
    });

    // Stage B: Writer thread - Immediately writes the events to disk
    let writer_thread = thread::spawn(move || -> anyhow::Result<()> {
        let mut writer = MemtrackWriter::new(out_file)?;

        let mut i = 0;
        while let Ok(first) = write_rx.recv() {
            writer.write_event(&first)?;
            i += 1;

            // Drain any backlog in a tight loop (batching)
            while let Ok(ev) = write_rx.try_recv() {
                writer.write_event(&ev)?;
                i += 1;
            }
        }
        writer.finish()?;

        info!("Wrote {i} memtrack events to disk");

        Ok(())
    });

    // Wait for the command to complete
    let status = child.wait().context("Failed to wait for command")?;
    debug!("Command exited with status: {status}");

    // Wait for drain thread to finish
    debug!("Waiting for the drain thread to finish");
    DRAIN_EVENTS.store(false, Ordering::Relaxed);
    drain_thread
        .join()
        .map_err(|_| anyhow::anyhow!("Failed to join drain thread"))?;

    // Wait for writer thread to finish and propagate errors
    debug!("Waiting for the writer thread to finish");
    drop(write_tx);
    writer_thread
        .join()
        .map_err(|_| anyhow::anyhow!("Failed to join writer thread"))??;

    // Read the eBPF dropped-event counter after the run is complete.
    // A non-zero value means the ring buffer overflowed and the trace is
    // incomplete.
    let dropped_events = tracker_arc
        .lock()
        .map_err(|_| anyhow!("tracker mutex poisoned"))?
        .dropped_events_count()
        .context("Failed to read memtrack dropped-event counter")?;
    if dropped_events > 0 {
        bail!(
            "Memtrack ring buffer overflowed: {dropped_events} events lost, aborting since the trace is incomplete.\n\
               Try reducing the benchmark's allocation rate (fewer iterations or smaller inputs), \
               or report it at https://github.com/CodSpeedHQ/codspeed/issues."
        );
    }

    // IPC thread will exit when channel closes
    drop(ipc_handle);

    Ok(status)
}
