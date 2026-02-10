use crate::prelude::*;
use anyhow::Context;
use futures::StreamExt;
use nix::{sys::time::TimeValLike, time::clock_gettime};
use runner_shared::artifacts::ExecutionTimestamps;
use runner_shared::fifo::{Command as FifoCommand, MarkerType};
use runner_shared::fifo::{RUNNER_ACK_FIFO, RUNNER_CTL_FIFO};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::{collections::HashSet, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio::net::unix::pid_t;
use tokio::net::unix::pipe::OpenOptions as TokioPipeOpenOptions;
use tokio::net::unix::pipe::Receiver as TokioPipeReader;
use tokio::net::unix::pipe::Sender as TokioPipeSender;
use tokio::time::error::Elapsed;
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};

fn create_fifo<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<()> {
    // Remove the previous FIFO (if it exists)
    let _ = nix::unistd::unlink(path.as_ref());

    // Create the FIFO with RWX permissions for the owner
    nix::unistd::mkfifo(path.as_ref(), nix::sys::stat::Mode::S_IRWXU)?;

    Ok(())
}

pub struct GenericFifo {
    ctl_path: PathBuf,
    ack_path: PathBuf,
    ctl_sender: TokioPipeSender,
    ack_reader: TokioPipeReader,
}

impl GenericFifo {
    pub fn new(ctl_fifo: &Path, ack_fifo: &Path) -> anyhow::Result<Self> {
        create_fifo(ctl_fifo)?;
        create_fifo(ack_fifo)?;

        let ctl_sender = get_pipe_open_options().open_sender(ctl_fifo)?;
        let ack_reader = get_pipe_open_options().open_receiver(ack_fifo)?;

        Ok(Self {
            ctl_path: ctl_fifo.to_path_buf(),
            ack_path: ack_fifo.to_path_buf(),
            ctl_sender,
            ack_reader,
        })
    }

    pub fn ctl_sender(&mut self) -> &mut TokioPipeSender {
        &mut self.ctl_sender
    }

    pub fn ack_reader(&mut self) -> &mut TokioPipeReader {
        &mut self.ack_reader
    }

    pub fn ctl_path(&self) -> &Path {
        &self.ctl_path
    }

    pub fn ack_path(&self) -> &Path {
        &self.ack_path
    }
}

pub struct FifoBenchmarkData {
    /// Name and version of the integration
    pub integration: Option<(String, String)>,
    pub bench_pids: HashSet<pid_t>,
}

impl FifoBenchmarkData {
    pub fn is_exec_harness(&self) -> bool {
        self.integration
            .as_ref()
            .is_some_and(|(name, _)| name == "exec-harness")
    }
}

pub struct RunnerFifo {
    ack_fifo: TokioPipeSender,
    ctl_reader: FramedRead<TokioPipeReader, LengthDelimitedCodec>,
}

fn get_pipe_open_options() -> TokioPipeOpenOptions {
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut options = TokioPipeOpenOptions::new();
    #[cfg(target_os = "linux")]
    options.read_write(true);
    options
}

impl RunnerFifo {
    pub fn new() -> anyhow::Result<Self> {
        Self::open(RUNNER_CTL_FIFO.as_ref(), RUNNER_ACK_FIFO.as_ref())
    }

    pub fn open(ctl_path: &Path, ack_path: &Path) -> anyhow::Result<Self> {
        create_fifo(ctl_path)?;
        create_fifo(ack_path)?;

        let ack_fifo = get_pipe_open_options().open_sender(ack_path)?;
        let ctl_fifo = get_pipe_open_options().open_receiver(ctl_path)?;

        let codec = LengthDelimitedCodec::builder()
            .length_field_length(4)
            .little_endian()
            .new_codec();
        let ctl_reader = FramedRead::new(ctl_fifo, codec);

        Ok(Self {
            ack_fifo,
            ctl_reader,
        })
    }

    pub async fn recv_cmd(&mut self) -> anyhow::Result<FifoCommand> {
        let bytes = self
            .ctl_reader
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("FIFO stream closed"))??;

        let decoded = bincode::deserialize(&bytes)
            .with_context(|| format!("Failed to deserialize FIFO command (data: {bytes:?})"))?;
        Ok(decoded)
    }

    pub async fn send_cmd(&mut self, cmd: FifoCommand) -> anyhow::Result<()> {
        let encoded = bincode::serialize(&cmd)?;

        self.ack_fifo
            .write_all(&(encoded.len() as u32).to_le_bytes())
            .await?;
        self.ack_fifo.write_all(&encoded).await?;
        Ok(())
    }

    /// Handles all incoming FIFO messages until it's closed, or until the child process exits.
    ///
    /// The `handle_cmd` callback is invoked first for each command. If it returns `Some(response)`,
    /// that response is sent and the shared implementation is skipped. If it returns `None`,
    /// the command falls through to the shared implementation for standard handling.
    ///
    /// Returns execution timestamps, benchmark data, and the exit status of the child process.
    pub async fn handle_fifo_messages(
        &mut self,
        child: &mut std::process::Child,
        mut handle_cmd: impl AsyncFnMut(&FifoCommand) -> anyhow::Result<Option<FifoCommand>>,
    ) -> anyhow::Result<(
        ExecutionTimestamps,
        FifoBenchmarkData,
        std::process::ExitStatus,
    )> {
        let mut bench_order_by_timestamp = Vec::<(u64, String)>::new();
        let mut bench_pids = HashSet::<pid_t>::new();
        let mut markers = Vec::<MarkerType>::new();

        let mut integration = None;

        let current_time = || {
            clock_gettime(nix::time::ClockId::CLOCK_MONOTONIC)
                .unwrap()
                .num_nanoseconds() as u64
        };

        let mut benchmark_started = false;

        // Outer loop: continues until health check fails
        loop {
            // Inner loop: process commands until timeout/error
            loop {
                let result: Result<_, Elapsed> =
                    tokio::time::timeout(Duration::from_secs(1), self.recv_cmd()).await;
                let cmd = match result {
                    Ok(Ok(cmd)) => cmd,
                    Ok(Err(e)) => {
                        warn!("Failed to parse FIFO command: {e}");
                        break;
                    }
                    Err(_) => break, // Timeout
                };
                trace!("Received command: {cmd:?}");

                // Try executor-specific handler first
                if let Some(response) = handle_cmd(&cmd).await? {
                    self.send_cmd(response).await?;
                    continue;
                }

                // Fall through to shared implementation for standard commands
                match &cmd {
                    FifoCommand::CurrentBenchmark { pid, uri } => {
                        bench_order_by_timestamp.push((current_time(), uri.to_string()));
                        bench_pids.insert(*pid);
                        self.send_cmd(FifoCommand::Ack).await?;
                    }
                    FifoCommand::StartBenchmark => {
                        if !benchmark_started {
                            benchmark_started = true;
                            markers.push(MarkerType::SampleStart(current_time()));
                        } else {
                            warn!("Received duplicate StartBenchmark command, ignoring");
                        }
                        self.send_cmd(FifoCommand::Ack).await?;
                    }
                    FifoCommand::StopBenchmark => {
                        if benchmark_started {
                            benchmark_started = false;
                            markers.push(MarkerType::SampleEnd(current_time()));
                        } else {
                            warn!("Received StopBenchmark command before StartBenchmark, ignoring");
                        }
                        self.send_cmd(FifoCommand::Ack).await?;
                    }
                    FifoCommand::SetIntegration { name, version } => {
                        integration = Some((name.into(), version.into()));
                        self.send_cmd(FifoCommand::Ack).await?;
                    }
                    FifoCommand::AddMarker { marker, .. } => {
                        markers.push(*marker);
                        self.send_cmd(FifoCommand::Ack).await?;
                    }
                    FifoCommand::SetVersion(protocol_version) => {
                        match protocol_version.cmp(&runner_shared::fifo::CURRENT_PROTOCOL_VERSION) {
                            Ordering::Less => {
                                if *protocol_version
                                    < runner_shared::fifo::MINIMAL_SUPPORTED_PROTOCOL_VERSION
                                {
                                    bail!(
                                        "Integration is using a version of the protocol that is smaller than the minimal supported protocol version ({protocol_version} < {}). \
                                        Please update the integration to a supported version.",
                                        runner_shared::fifo::MINIMAL_SUPPORTED_PROTOCOL_VERSION
                                    );
                                }
                                self.send_cmd(FifoCommand::Ack).await?;
                            }
                            Ordering::Greater => bail!(
                                "Runner is using an incompatible protocol version ({} < {protocol_version}). Please update the runner to the latest version.",
                                runner_shared::fifo::CURRENT_PROTOCOL_VERSION
                            ),
                            Ordering::Equal => {
                                self.send_cmd(FifoCommand::Ack).await?;
                            }
                        }
                    }
                    _ => {
                        warn!("Unhandled FIFO command: {cmd:?}");
                        self.send_cmd(FifoCommand::Err).await?;
                    }
                }
            }

            // Check if the process has exited using try_wait (non-blocking)
            match child.try_wait() {
                Ok(None) => {} // Still running, continue loop
                Ok(Some(exit_status)) => {
                    debug!(
                        "Process terminated with status: {exit_status}, stopping the command handler"
                    );
                    let marker_result =
                        ExecutionTimestamps::new(&bench_order_by_timestamp, &markers);
                    let fifo_data = FifoBenchmarkData {
                        integration,
                        bench_pids,
                    };
                    return Ok((marker_result, fifo_data, exit_status));
                }
                Err(e) => return Err(anyhow::Error::from(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn recv_cmd_is_not_cancel_safe() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ctl_path = temp_dir.path().join("ctl_fifo");
        let ack_path = temp_dir.path().join("ack_fifo");

        let mut fifo = RunnerFifo::open(&ctl_path, &ack_path).unwrap();
        let mut writer = get_pipe_open_options().open_sender(&ctl_path).unwrap();

        let cmd = FifoCommand::Ack;
        let payload = bincode::serialize(&cmd).unwrap();
        let len_bytes = (payload.len() as u32).to_le_bytes();

        tokio::spawn(async move {
            writer.write_all(&len_bytes).await.unwrap();
            writer.write_all(&payload[..1]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            writer.write_all(&payload[1..]).await.unwrap();
        });

        let first = tokio::time::timeout(Duration::from_millis(10), fifo.recv_cmd()).await;
        assert!(first.is_err(), "Expected timeout on first recv_cmd");

        let second = tokio::time::timeout(Duration::from_millis(200), fifo.recv_cmd()).await;

        assert!(
            matches!(second, Ok(Ok(FifoCommand::Ack))),
            "recv_cmd should be cancel-safe: expected Ok(Ok(Ack)), got: {second:?}"
        );
    }
}
