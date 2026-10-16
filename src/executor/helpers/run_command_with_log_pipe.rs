use crate::executor::EXECUTOR_TARGET;
use crate::local_logger::rolling_buffer::ROLLING_BUFFER;
use crate::local_logger::suspend_progress_bar;
use crate::prelude::*;
use std::fs::File;
use std::future::Future;
use std::io::{Read, Write};
use std::process::ExitStatus;
use tokio::process::Command;

/// Take the child's piped stdout and stderr as blocking files, for reading on plain threads.
pub fn take_blocking_stdio(child: &mut tokio::process::Child) -> Result<(File, File)> {
    let stdout = child.stdout.take().context("child stdout is not piped")?;
    let stderr = child.stderr.take().context("child stderr is not piped")?;
    Ok((
        File::from(stdout.into_owned_fd()?),
        File::from(stderr.into_owned_fd()?),
    ))
}

/// Run a command and log its output to stdout and stderr
///
/// # Arguments
/// - `cmd`: The command to run.
/// - `cb`: A callback function that takes the process and returns the exit status.
///
/// # Returns
///
/// The exit status of the command.
///
pub async fn run_command_with_log_pipe_and_callback<F, Fut>(
    mut cmd: Command,
    cb: F,
) -> Result<ExitStatus>
where
    F: FnOnce(tokio::process::Child) -> Fut,
    Fut: Future<Output = anyhow::Result<ExitStatus>>,
{
    /// Write text to the rolling buffer if active, otherwise write raw bytes to the writer.
    fn write_to_rolling_buffer_or_output(text: &str, raw_bytes: &[u8], writer: &mut impl Write) {
        if let Ok(mut guard) = ROLLING_BUFFER.lock() {
            if let Some(rb) = guard.as_mut() {
                if rb.is_active() {
                    rb.push_lines(text);
                    return;
                }
            }
        }
        suspend_progress_bar(|| writer.write_all(raw_bytes).unwrap());
    }

    fn log_tee(
        mut reader: impl Read,
        mut writer: impl Write,
        log_prefix: Option<&str>,
    ) -> Result<()> {
        let prefix = log_prefix.unwrap_or("");
        let mut buffer = [0; 1024];
        let mut line_buffer = Vec::new();

        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                // Flush any remaining data in the line buffer
                if !line_buffer.is_empty() {
                    let text = String::from_utf8_lossy(&line_buffer);
                    trace!(target: EXECUTOR_TARGET, "{prefix}{text}");
                    write_to_rolling_buffer_or_output(&text, &line_buffer, &mut writer);
                }
                break;
            }

            // Add the chunk to our line buffer
            line_buffer.extend_from_slice(&buffer[..bytes_read]);

            // Check if we have any complete lines (ending with \n or \r)
            if let Some(last_newline_pos) =
                line_buffer.iter().rposition(|&b| b == b'\n' || b == b'\r')
            {
                // We have at least one complete line, flush up to and including the last newline
                let to_flush = &line_buffer[..=last_newline_pos];
                let text = String::from_utf8_lossy(to_flush);
                trace!(target: EXECUTOR_TARGET, "{prefix}{text}");
                write_to_rolling_buffer_or_output(&text, to_flush, &mut writer);

                // Keep the remainder in the buffer
                line_buffer = line_buffer[last_newline_pos + 1..].to_vec();
            }
        }
        Ok(())
    }

    let mut process = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn the process")?;
    let (stdout, stderr) = take_blocking_stdio(&mut process)?;

    let stdout_handle = tokio::task::spawn_blocking(move || {
        log_tee(stdout, std::io::stdout(), None).unwrap();
    });

    let stderr_handle = tokio::task::spawn_blocking(move || {
        log_tee(stderr, std::io::stderr(), Some("[stderr]")).unwrap();
    });

    let result = cb(process).await;

    // Wait for threads to drain remaining output
    let _ = stdout_handle.await;
    let _ = stderr_handle.await;

    result
}

pub async fn run_command_with_log_pipe(cmd: Command) -> Result<ExitStatus> {
    run_command_with_log_pipe_and_callback(cmd, |mut child| async move { Ok(child.wait().await?) })
        .await
}
