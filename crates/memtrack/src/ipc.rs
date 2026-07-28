use crate::prelude::*;
use ipc_channel::ipc::{self, IpcOneShotServer, IpcSender};
use serde::{Deserialize, Serialize};

#[cfg(feature = "ebpf")]
use crate::ebpf::Tracker;

pub type MemtrackIpcServer = IpcOneShotServer<IpcSender<IpcMessage>>;

/// Commands sent from the runner to control memtrack
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcCommand {
    Enable,
    Disable,
    Ping,
}

/// Response sent back to runner
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcResponse {
    Ack,
    Err,
}

/// Message combining command and response channel
#[derive(Debug, Serialize, Deserialize)]
pub struct IpcMessage {
    pub command: IpcCommand,
    pub response_channel: IpcSender<IpcResponse>,
}

/// Client for sending commands from runner to memtrack
pub struct MemtrackIpcClient {
    sender: IpcSender<IpcMessage>,
}

impl MemtrackIpcClient {
    pub fn new(sender: IpcSender<IpcMessage>) -> Self {
        Self { sender }
    }

    /// Create from the connection accepted by the runner
    pub fn from_accepted(sender: IpcSender<IpcMessage>) -> Self {
        Self { sender }
    }

    fn send_command(&self, cmd: IpcCommand) -> Result<IpcResponse> {
        let (response_tx, response_rx) = ipc::channel::<IpcResponse>()?;

        let msg = IpcMessage {
            command: cmd,
            response_channel: response_tx,
        };

        self.sender.send(msg).context("Failed to send command")?;
        let response = response_rx.recv().context("Failed to receive response")?;

        Ok(response)
    }

    pub fn enable(&self) -> Result<()> {
        let response = self.send_command(IpcCommand::Enable)?;
        match response {
            IpcResponse::Ack => Ok(()),
            IpcResponse::Err => anyhow::bail!("Failed to enable tracking"),
        }
    }

    pub fn disable(&self) -> Result<()> {
        let response = self.send_command(IpcCommand::Disable)?;
        match response {
            IpcResponse::Ack => Ok(()),
            IpcResponse::Err => anyhow::bail!("Failed to disable tracking"),
        }
    }

    pub fn ping(&self) -> Result<()> {
        let response = self.send_command(IpcCommand::Ping)?;
        match response {
            IpcResponse::Ack => Ok(()),
            IpcResponse::Err => anyhow::bail!("Failed to ping memtrack"),
        }
    }
}

/// Handle incoming IPC messages in memtrack
#[cfg(feature = "ebpf")]
pub fn handle_ipc_message(msg: IpcMessage, tracker: &Tracker) {
    let response = match msg.command {
        IpcCommand::Enable => match tracker.enable_tracking() {
            Ok(_) => {
                debug!("Tracking enabled");
                IpcResponse::Ack
            }
            Err(_) => IpcResponse::Err,
        },
        IpcCommand::Disable => match tracker.disable_tracking() {
            Ok(_) => {
                debug!("Tracking disabled");
                IpcResponse::Ack
            }
            Err(_) => IpcResponse::Err,
        },
        IpcCommand::Ping => IpcResponse::Ack,
    };

    // Send response back
    let _ = msg.response_channel.send(response);
}
