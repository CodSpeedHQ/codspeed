use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::fs;

use crate::prelude::*;

use super::PostgresConfig;

/// Collects the analytics dump produced by the `codspeed/postgres` image's
/// poller and drops it into the profile folder so it is uploaded with the run.
///
/// Unlike the MongoDB instrument, this owns no process and does no proxying: the
/// poller runs inside the database image and writes the dump to a file, so this
/// only copies that file into the profile folder.
#[derive(Debug)]
pub struct PostgresInstrument {
    profile_folder: PathBuf,
    dump_path: PathBuf,
    max_flush_wait: Duration,
    flush_poll_interval: Duration,
}

/// Bound on how long to wait for the poller to flush a fresh dump after the
/// benchmark ends, and how often to re-check. The poller flushes on a fixed
/// interval (2s by default), so the bound is a few of those.
const MAX_FLUSH_WAIT: Duration = Duration::from_secs(6);
const FLUSH_POLL_INTERVAL: Duration = Duration::from_millis(200);

impl PostgresInstrument {
    pub fn new(profile_folder: &Path, config: &PostgresConfig) -> Self {
        Self {
            profile_folder: profile_folder.to_path_buf(),
            dump_path: config.dump_path.clone(),
            max_flush_wait: MAX_FLUSH_WAIT,
            flush_poll_interval: FLUSH_POLL_INTERVAL,
        }
    }

    /// Copy the dump into `<profile_folder>/instruments/postgres.json`, so it
    /// rides along in the uploaded profile archive. A missing dump is a warning,
    /// not a failure — a benchmark run must not fail over missing analytics.
    pub async fn collect(&self) -> Result<()> {
        // The benchmark has just finished. The poller flushes the dump on a fixed
        // interval, so copying it immediately would drop up to one interval of the
        // final queries. Wait for a flush that happened after now (the run end).
        self.wait_for_fresh_dump(SystemTime::now()).await;

        if !self.dump_path.exists() {
            warn!(
                "Postgres instrument enabled but no dump found at {}; skipping",
                self.dump_path.display()
            );
            return Ok(());
        }

        let instruments_out_dir = self.profile_folder.join("instruments");
        fs::create_dir_all(&instruments_out_dir).await?;
        let dest = instruments_out_dir.join("postgres.json");
        fs::copy(&self.dump_path, &dest).await.with_context(|| {
            format!(
                "Failed to copy Postgres dump from {}",
                self.dump_path.display()
            )
        })?;
        debug!("Collected Postgres analytics into {}", dest.display());

        Ok(())
    }

    /// Poll the dump's mtime until it advances past `after`, so the copied dump
    /// reflects a poller flush that happened after the last benchmark query. The
    /// poller writes atomically (temp file + rename), so a bumped mtime means a
    /// complete document. Bounded: the poller may already be idle or gone.
    async fn wait_for_fresh_dump(&self, after: SystemTime) {
        let deadline = tokio::time::Instant::now() + self.max_flush_wait;
        while tokio::time::Instant::now() < deadline {
            if let Ok(meta) = fs::metadata(&self.dump_path).await {
                if let Ok(mtime) = meta.modified() {
                    if mtime >= after {
                        return;
                    }
                }
            }
            tokio::time::sleep(self.flush_poll_interval).await;
        }
        warn!(
            "Postgres dump at {} did not refresh within {:?}; using the latest available snapshot",
            self.dump_path.display(),
            self.max_flush_wait
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::id;

    impl PostgresInstrument {
        fn with_timing(mut self, max_flush_wait: Duration, flush_poll_interval: Duration) -> Self {
            self.max_flush_wait = max_flush_wait;
            self.flush_poll_interval = flush_poll_interval;
            self
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pg-instr-{}-{tag}", id()))
    }

    fn instrument(profile: &Path, dump_path: PathBuf, max: Duration) -> PostgresInstrument {
        PostgresInstrument::new(profile, &PostgresConfig { dump_path })
            .with_timing(max, Duration::from_millis(20))
    }

    /// collect() must not copy the dump the poller last flushed before the run
    /// ended: it waits for a flush newer than the run end, so the final queries
    /// are captured instead of dropped by the poller's tick interval.
    #[tokio::test]
    async fn collect_waits_for_post_run_flush() {
        let base = scratch("fresh");
        let _ = tokio::fs::remove_dir_all(&base).await;
        tokio::fs::create_dir_all(&base).await.unwrap();
        let dump_path = base.join("dump.json");
        let profile = base.join("pf");
        tokio::fs::write(&dump_path, br#"{"queries":[]}"#)
            .await
            .unwrap();

        let instr = instrument(&profile, dump_path.clone(), Duration::from_secs(6));

        let dp = dump_path.clone();
        let flush = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            tokio::fs::write(&dp, br#"{"queries":[{"sql":"select 1"}]}"#)
                .await
                .unwrap();
        });

        instr.collect().await.unwrap();
        flush.await.unwrap();

        let copied = tokio::fs::read_to_string(profile.join("instruments/postgres.json"))
            .await
            .unwrap();
        assert!(
            copied.contains("select 1"),
            "collect copied a stale pre-flush dump: {copied}"
        );
        let _ = tokio::fs::remove_dir_all(&base).await;
    }

    /// A missing dump is not a failure — a benchmark run must not fail over
    /// missing analytics — and nothing is written.
    #[tokio::test]
    async fn collect_missing_dump_is_ok_and_writes_nothing() {
        let base = scratch("missing");
        let _ = tokio::fs::remove_dir_all(&base).await;
        tokio::fs::create_dir_all(&base).await.unwrap();
        let profile = base.join("pf");

        let instr = instrument(
            &profile,
            base.join("does-not-exist.json"),
            Duration::from_millis(100),
        );

        instr.collect().await.unwrap();
        assert!(
            !profile.join("instruments/postgres.json").exists(),
            "no dump should be written when the source is missing"
        );
        let _ = tokio::fs::remove_dir_all(&base).await;
    }
}
