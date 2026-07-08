use clap::Parser;
use ipc_channel::ipc;
use memtrack::prelude::*;
use memtrack::{MemtrackIpcMessage, Tracker, handle_ipc_message};
use runner_shared::artifacts::{ArtifactExt, MemtrackArtifact, encode_events};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;

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

    let tracker = Arc::new(Tracker::new()?);

    // Spawn IPC handler thread with the now-available tracker
    let ipc_handle = if let Some(rx) = ipc_channel {
        let tracker = tracker.clone();
        Some(thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                handle_ipc_message(msg, &tracker);
            }
        }))
    } else {
        // Without IPC, nothing toggles the tracking_enabled map, so events would
        // be dropped by the eBPF is_enabled() check. Enable it up front.
        tracker.enable_tracking()?;
        None
    };

    // Run the target command through bash to handle shell syntax. Drop
    // privileges if running under sudo to avoid permission issues when the
    // target accesses files owned by the original user.
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(cmd_string);
    let uid_gid = get_user_uid_gid();
    if let Some((uid, gid)) = uid_gid {
        debug!("Running under sudo, dropping privileges to uid={uid}, gid={gid}");
    }

    let mut session = tracker
        .spawn(&cmd, uid_gid)
        .map_err(|e| anyhow!("Failed to spawn child process: {e}"))?;
    let root_pid = session.pid();
    let event_rx = session.take_events()?;
    debug!("Spawned child with pid {root_pid}");

    // Generate output file name and create file for streaming events
    let file_name = MemtrackArtifact::file_name(Some(root_pid));
    let out_file = std::fs::File::create(out_dir.join(file_name))?;

    // Leave headroom for the ring buffer poll thread and the tracked
    // command: encode workers on every core starve the poller during
    // allocation bursts, which overflows the kernel ring buffer.
    let n_workers = thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2).max(1))
        .unwrap_or(4);

    let pipeline_thread = thread::spawn(move || encode_events(event_rx, out_file, n_workers));

    // Wait for the command to complete
    let status = session.wait().context("Failed to wait for command")?;
    debug!("Command exited with status: {status}");

    // Stop event production before draining: the child has exited, so anything
    // still arriving is already in the ring buffer.
    if let Err(e) = tracker.disable_tracking() {
        warn!("Failed to disable tracking: {e:#}");
    }

    // Dropping the session drops the event poller, which does a final drain of
    // the ring buffer and then closes the event channel. Without this the
    // encode pipeline join below would block forever.
    debug!("Stopping the ring buffer poller");
    drop(session);

    debug!("Waiting for the encode pipeline to finish");
    let total = pipeline_thread
        .join()
        .map_err(|_| anyhow::anyhow!("Failed to join memtrack encode pipeline"))??;

    info!("Wrote {total} memtrack events to disk");

    // Stop the attach worker and surface any fatal error it recorded (missed
    // exec mappings mean incomplete allocator coverage).
    tracker.finish()?;

    // Detach probes explicitly: the IPC thread still holds an Arc clone, so the
    // tracker would otherwise never be dropped before process::exit and the
    // kernel would close every link fd serially during exit.
    tracker.detach();

    // Read the eBPF dropped-event counter after the run is complete.
    // A non-zero value means the ring buffer overflowed and the trace is
    // incomplete.
    let dropped_events = tracker
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
