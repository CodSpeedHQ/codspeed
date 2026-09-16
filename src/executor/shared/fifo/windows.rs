use crate::prelude::*;
use anyhow::Context;
use futures::StreamExt;
use runner_shared::artifacts::ExecutionTimestamps;
use runner_shared::fifo::{Command as FifoCommand, MarkerType};
use runner_shared::fifo::{RUNNER_ACK_FIFO, RUNNER_CTL_FIFO};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::time::error::Elapsed;
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};

const ERROR_PIPE_CONNECTED: i32 = 535;

pub struct FifoBenchmarkData {
    pub integration: Option<(String, String)>,
    pub bench_pids: HashSet<i32>,
}

impl FifoBenchmarkData {
    pub fn is_exec_harness(&self) -> bool {
        self.integration
            .as_ref()
            .is_some_and(|(name, _)| name == "exec-harness")
    }
}

pub struct RunnerFifo {
    ack_pipe: Option<NamedPipeServer>,
    ctl_pipe: Option<NamedPipeServer>,
    ctl_reader: Option<FramedRead<NamedPipeServer, LengthDelimitedCodec>>,
}

impl RunnerFifo {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            ack_pipe: Some(ServerOptions::new().create(RUNNER_ACK_FIFO)?),
            ctl_pipe: Some(ServerOptions::new().create(RUNNER_CTL_FIFO)?),
            ctl_reader: None,
        })
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if self.ctl_reader.is_some() {
            return Ok(());
        }

        let mut ctl_pipe = self
            .ctl_pipe
            .take()
            .context("control pipe is already connected")?;
        let mut ack_pipe = self
            .ack_pipe
            .take()
            .context("acknowledgement pipe is already connected")?;
        connect_pipe(&mut ctl_pipe).await?;
        connect_pipe(&mut ack_pipe).await?;

        let codec = LengthDelimitedCodec::builder()
            .length_field_length(4)
            .little_endian()
            .new_codec();
        self.ctl_reader = Some(FramedRead::new(ctl_pipe, codec));
        self.ack_pipe = Some(ack_pipe);
        Ok(())
    }

    pub async fn recv_cmd(&mut self) -> anyhow::Result<FifoCommand> {
        self.connect().await?;
        let bytes = self
            .ctl_reader
            .as_mut()
            .expect("control pipe is connected")
            .next()
            .await
            .ok_or_else(|| anyhow!("named-pipe stream closed"))??;

        bincode::deserialize(&bytes)
            .with_context(|| format!("Failed to deserialize FIFO command (data: {bytes:?})"))
    }

    pub async fn send_cmd(&mut self, cmd: FifoCommand) -> anyhow::Result<()> {
        self.connect().await?;
        let encoded = bincode::serialize(&cmd)?;
        let ack_pipe = self
            .ack_pipe
            .as_mut()
            .expect("acknowledgement pipe is connected");
        ack_pipe
            .write_all(&(encoded.len() as u32).to_le_bytes())
            .await?;
        ack_pipe.write_all(&encoded).await?;
        Ok(())
    }

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
        let mut bench_pids = HashSet::<i32>::new();
        let mut markers = Vec::<MarkerType>::new();
        let mut integration = None;
        let get_current_time = instrument_hooks_bindings::InstrumentHooks::current_timestamp;
        let mut benchmark_started = false;

        loop {
            loop {
                let result: Result<_, Elapsed> =
                    tokio::time::timeout(Duration::from_secs(1), self.recv_cmd()).await;
                let cmd = match result {
                    Ok(Ok(cmd)) => cmd,
                    Ok(Err(error)) => {
                        warn!("Failed to parse named-pipe command: {error}");
                        break;
                    }
                    Err(_) => break,
                };
                trace!("Received command: {cmd:?}");

                if let Some(response) = handle_cmd(&cmd).await? {
                    self.send_cmd(response).await?;
                    continue;
                }

                match &cmd {
                    FifoCommand::CurrentBenchmark { pid, uri } => {
                        bench_order_by_timestamp.push((get_current_time(), uri.to_string()));
                        bench_pids.insert(*pid);
                        self.send_cmd(FifoCommand::Ack).await?;
                    }
                    FifoCommand::StartProfiler => {
                        if !benchmark_started {
                            benchmark_started = true;
                            markers.push(MarkerType::SampleStart(get_current_time()));
                        } else {
                            warn!("Received duplicate StartProfiler command, ignoring");
                        }
                        self.send_cmd(FifoCommand::Ack).await?;
                    }
                    FifoCommand::StopProfiler => {
                        if benchmark_started {
                            benchmark_started = false;
                            markers.push(MarkerType::SampleEnd(get_current_time()));
                        } else {
                            warn!("Received StopProfiler command before StartProfiler, ignoring");
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
                            Ordering::Less
                                if *protocol_version
                                    < runner_shared::fifo::MINIMAL_SUPPORTED_PROTOCOL_VERSION =>
                            {
                                bail!(
                                    "Integration protocol version {protocol_version} is older than the minimum supported version {}",
                                    runner_shared::fifo::MINIMAL_SUPPORTED_PROTOCOL_VERSION
                                )
                            }
                            Ordering::Greater => bail!(
                                "Runner protocol version {} is older than integration protocol version {protocol_version}",
                                runner_shared::fifo::CURRENT_PROTOCOL_VERSION
                            ),
                            _ => self.send_cmd(FifoCommand::Ack).await?,
                        }
                    }
                    _ => {
                        warn!("Unhandled FIFO command: {cmd:?}");
                        self.send_cmd(FifoCommand::Err).await?;
                    }
                }
            }

            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(exit_status)) => {
                    let timestamps = ExecutionTimestamps::new(&bench_order_by_timestamp, &markers);
                    return Ok((
                        timestamps,
                        FifoBenchmarkData {
                            integration,
                            bench_pids,
                        },
                        exit_status,
                    ));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

async fn connect_pipe(pipe: &mut NamedPipeServer) -> anyhow::Result<()> {
    if let Err(error) = pipe.connect().await {
        if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED) {
            return Err(error.into());
        }
    }
    Ok(())
}
