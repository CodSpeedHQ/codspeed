use std::ops::Deref;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::executor::shared::fifo::GenericFifo;
pub struct PerfFifo {
    fifo: GenericFifo,
}

impl PerfFifo {
    pub fn new() -> anyhow::Result<Self> {
        let fifo_dir = tempfile::tempdir()?.keep();
        let fifo = GenericFifo::new(
            &fifo_dir.join("codspeed_perf.ctl.fifo"),
            &fifo_dir.join("codspeed_perf.ack.fifo"),
        )?;
        Ok(Self { fifo })
    }

    pub async fn start_events(&mut self) -> anyhow::Result<()> {
        self.fifo.ctl_sender().write_all(b"enable\n\0").await?;
        self.wait_for_ack().await;

        Ok(())
    }

    pub async fn stop_events(&mut self) -> anyhow::Result<()> {
        self.fifo.ctl_sender().write_all(b"disable\n\0").await?;
        self.wait_for_ack().await;

        Ok(())
    }

    pub async fn ping(&mut self) -> anyhow::Result<()> {
        self.fifo.ctl_sender().write_all(b"ping\n\0").await?;
        self.wait_for_ack().await;

        Ok(())
    }

    async fn wait_for_ack(&mut self) {
        const ACK: &[u8] = b"ack\n\0";

        loop {
            let mut buf: [u8; ACK.len()] = [0; ACK.len()];
            if self.fifo.ack_reader().read_exact(&mut buf).await.is_err() {
                continue;
            }

            if buf == ACK {
                break;
            }
        }
    }
}

impl Deref for PerfFifo {
    type Target = GenericFifo;

    fn deref(&self) -> &Self::Target {
        &self.fifo
    }
}
