use std::collections::{HashMap, HashSet};
use std::path::Path;

use runner_shared::artifacts::ExecutionTimestamps;
use serde::Serialize;
use serde_json::Value;
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

use crate::prelude::*;

use super::PostgresConfig;

/// One statement's `pg_stat_statements` counters plus its plan (attached at
/// finalize). This is the per-query shape written into the artifact.
#[derive(Debug, Clone, Serialize)]
pub struct PostgresQuery {
    pub sql: String,
    pub calls: i64,
    pub rows: i64,
    pub shared_blks_hit: i64,
    pub shared_blks_read: i64,
    /// EXPLAIN plan tree, or `null` when the statement can't be explained.
    pub plan: Value,
}

/// The queries a single benchmark issued, keyed to its URI (the same URI as its
/// flamegraph region).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BenchmarkQueries {
    pub uri: String,
    pub queries: Vec<PostgresQuery>,
}

#[derive(Debug, Serialize)]
struct PostgresDump {
    benchmarks: Vec<BenchmarkQueries>,
}

// so BenchmarkQueries can derive PartialEq for the zip test
impl PartialEq for PostgresQuery {
    fn eq(&self, other: &Self) -> bool {
        self.sql == other.sql
            && self.calls == other.calls
            && self.rows == other.rows
            && self.shared_blks_hit == other.shared_blks_hit
            && self.shared_blks_read == other.shared_blks_read
            && self.plan == other.plan
    }
}

/// Drives per-benchmark `pg_stat_statements` capture on the runner's own
/// (superuser) connection, keyed to benchmark boundaries.
///
/// Unlike the whole-run poller it replaces, this resets at each benchmark start
/// and snapshots at each benchmark stop — driven from the FIFO boundary hooks so
/// the SQL runs outside the measured region — then keys each snapshot to the
/// benchmark URI (the same one its flamegraph uses) at finalize.
pub struct PostgresInstrument {
    client: Client,
    /// One counter snapshot per benchmark, in stop-boundary (execution) order.
    snapshots: Vec<Vec<PostgresQuery>>,
    /// EXPLAIN plan cache keyed by normalized SQL — a plan does not change during
    /// a run, so each statement is explained at most once.
    plans: HashMap<String, Value>,
}

impl PostgresInstrument {
    /// Resolve the DSN from its env var, connect, and ensure the extension exists.
    pub async fn connect(config: &PostgresConfig) -> Result<Self> {
        let dsn = std::env::var(&config.dsn_env_name).with_context(|| {
            format!(
                "reading the Postgres DSN from ${} (--postgres-dsn-env-name)",
                config.dsn_env_name
            )
        })?;
        let (client, connection) = tokio_postgres::connect(&dsn, NoTls)
            .await
            .context("connecting the Postgres instrument to the database")?;
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                warn!("Postgres instrument connection closed: {err}");
            }
        });
        client
            .simple_query("CREATE EXTENSION IF NOT EXISTS pg_stat_statements")
            .await
            .context("creating pg_stat_statements extension")?;
        Ok(Self {
            client,
            snapshots: Vec::new(),
            plans: HashMap::new(),
        })
    }

    /// Reset `pg_stat_statements` at a benchmark start, so the next snapshot holds
    /// only this benchmark's statements.
    pub async fn reset(&self) -> Result<()> {
        self.client
            .simple_query("SELECT pg_stat_statements_reset()")
            .await
            .context("resetting pg_stat_statements")?;
        Ok(())
    }

    /// Snapshot the current counters at a benchmark stop and buffer them (one
    /// entry per benchmark, in execution order).
    pub async fn snapshot(&mut self) -> Result<()> {
        let rows = self
            .client
            .query(
                "SELECT s.query, s.calls, s.rows, s.shared_blks_hit, s.shared_blks_read \
                 FROM pg_stat_statements s JOIN pg_database d ON d.oid = s.dbid \
                 WHERE d.datname = current_database() ORDER BY s.calls DESC, s.query",
                &[],
            )
            .await
            .context("reading pg_stat_statements")?;

        let queries = rows
            .into_iter()
            .filter_map(|row| {
                let sql: String = row.get(0);
                if is_self_query(&sql) {
                    return None;
                }
                Some(PostgresQuery {
                    sql,
                    calls: row.get(1),
                    rows: row.get(2),
                    shared_blks_hit: row.get(3),
                    shared_blks_read: row.get(4),
                    plan: Value::Null,
                })
            })
            .collect();
        self.snapshots.push(queries);
        Ok(())
    }

    /// EXPLAIN each distinct statement, key the buffered snapshots to benchmark
    /// URIs (via `uri_by_ts`, the same zipping the flamegraph uses), and write the
    /// per-benchmark artifact to `<profile_folder>/instruments/postgres.json`.
    pub async fn finalize(
        &mut self,
        timestamps: &ExecutionTimestamps,
        profile_folder: &Path,
    ) -> Result<()> {
        self.attach_plans().await;

        // On a URI/snapshot count mismatch, skip the artifact entirely rather than
        // zip by index — that would misattribute queries to the wrong benchmark,
        // which is worse than emitting nothing (nothing downstream can detect it).
        let Some(benchmarks) = zip_benchmarks(&timestamps.uri_by_ts, &self.snapshots) else {
            return Ok(());
        };
        let count = benchmarks.len();

        let out_dir = profile_folder.join("instruments");
        tokio::fs::create_dir_all(&out_dir)
            .await
            .with_context(|| format!("creating {}", out_dir.display()))?;
        let dest = out_dir.join("postgres.json");
        let tmp = out_dir.join("postgres.json.tmp");
        let bytes = serde_json::to_vec_pretty(&PostgresDump { benchmarks })?;
        tokio::fs::write(&tmp, &bytes)
            .await
            .with_context(|| format!("writing {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &dest)
            .await
            .with_context(|| format!("renaming {} to {}", tmp.display(), dest.display()))?;

        debug!(
            "Collected Postgres analytics for {count} benchmark(s) into {}",
            dest.display()
        );
        Ok(())
    }

    /// Fill each buffered query's `plan` from the EXPLAIN cache, explaining any
    /// not-yet-seen statement once.
    async fn attach_plans(&mut self) {
        let distinct: HashSet<String> = self
            .snapshots
            .iter()
            .flat_map(|snap| snap.iter().map(|q| q.sql.clone()))
            .collect();
        for sql in distinct {
            if self.plans.contains_key(&sql) {
                continue;
            }
            let plan = match explain(&self.client, &sql).await {
                Ok(plan) => plan,
                Err(err) => {
                    debug!("EXPLAIN skipped for `{sql}`: {err:#}");
                    Value::Null
                }
            };
            self.plans.insert(sql, plan);
        }
        for snap in &mut self.snapshots {
            for query in snap.iter_mut() {
                query.plan = self.plans.get(&query.sql).cloned().unwrap_or(Value::Null);
            }
        }
    }
}

/// Zip per-benchmark snapshots to their URIs in execution order — mirroring how
/// the flamegraph keys each `SampleStart..SampleEnd` region to `uri_by_ts`.
fn zip_benchmarks(
    uri_by_ts: &[(u64, String)],
    snapshots: &[Vec<PostgresQuery>],
) -> Option<Vec<BenchmarkQueries>> {
    if uri_by_ts.len() != snapshots.len() {
        warn!(
            "Postgres: {} benchmark URIs but {} snapshots; skipping artifact to avoid misattribution",
            uri_by_ts.len(),
            snapshots.len()
        );
        return None;
    }
    Some(
        uri_by_ts
            .iter()
            .zip(snapshots.iter())
            .map(|((_, uri), queries)| BenchmarkQueries {
                uri: uri.clone(),
                queries: queries.clone(),
            })
            .collect(),
    )
}

/// The instrument's own bookkeeping queries, which must not appear in the dump.
fn is_self_query(sql: &str) -> bool {
    let sql = sql.trim_start();
    sql.starts_with("EXPLAIN") || sql.contains("pg_stat_statements")
}

/// Run `EXPLAIN` for `sql` and return the plan tree. Uses `GENERIC_PLAN` (PG16+)
/// when the statement is parameterized — `pg_stat_statements` normalizes literals
/// to `$1`, which can't be planned otherwise — over `simple_query` so the `$1`
/// stays part of the explained statement rather than becoming an EXPLAIN param.
async fn explain(client: &Client, sql: &str) -> Result<Value> {
    let options = if has_placeholder(sql) {
        "GENERIC_PLAN, FORMAT JSON"
    } else {
        "FORMAT JSON"
    };
    let messages = client
        .simple_query(&format!("EXPLAIN ({options}) {sql}"))
        .await
        .context("running EXPLAIN")?;
    for msg in messages {
        if let SimpleQueryMessage::Row(row) = msg {
            let text = row.get(0).context("EXPLAIN row missing plan column")?;
            return serde_json::from_str(text).context("parsing EXPLAIN JSON");
        }
    }
    bail!("EXPLAIN returned no rows");
}

/// Whether `sql` contains a `$N` parameter placeholder.
fn has_placeholder(sql: &str) -> bool {
    sql.as_bytes()
        .windows(2)
        .any(|w| w[0] == b'$' && w[1].is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(sql: &str, calls: i64) -> PostgresQuery {
        PostgresQuery {
            sql: sql.into(),
            calls,
            rows: 0,
            shared_blks_hit: 0,
            shared_blks_read: 0,
            plan: Value::Null,
        }
    }

    #[test]
    fn zips_snapshots_to_uris_in_order() {
        let uri_by_ts = vec![(10, "bench::a".to_string()), (20, "bench::b".to_string())];
        let snapshots = vec![vec![q("select 1", 3)], vec![q("select 2", 5)]];

        let out = zip_benchmarks(&uri_by_ts, &snapshots).unwrap();

        assert_eq!(
            out,
            vec![
                BenchmarkQueries {
                    uri: "bench::a".into(),
                    queries: vec![q("select 1", 3)],
                },
                BenchmarkQueries {
                    uri: "bench::b".into(),
                    queries: vec![q("select 2", 5)],
                },
            ]
        );
    }

    #[test]
    fn zip_skips_on_length_mismatch() {
        let uri_by_ts = vec![(10, "bench::a".to_string())];
        let snapshots = vec![vec![q("select 1", 1)], vec![q("select 2", 2)]];
        // A mismatch must yield None (skip the artifact) rather than misattribute.
        assert!(zip_benchmarks(&uri_by_ts, &snapshots).is_none());
    }

    #[test]
    fn detects_parameter_placeholders() {
        assert!(has_placeholder("select * from t where id = $1"));
        assert!(!has_placeholder("select * from t"));
        assert!(!has_placeholder("select '$x' from t"));
    }

    #[test]
    fn skips_self_queries() {
        assert!(is_self_query("EXPLAIN (FORMAT JSON) select 1"));
        assert!(is_self_query("SELECT pg_stat_statements_reset()"));
        assert!(!is_self_query("select * from users"));
    }

    /// Isolation invariant against a real database: `reset()` at each benchmark
    /// boundary must prevent one benchmark's queries from leaking into the next,
    /// and `EXPLAIN (GENERIC_PLAN)` must capture a plan for `$1`-parameterized SQL.
    ///
    /// A no-op unless `PG_SMOKE_DSN` is set. To run it:
    ///   docker run -d --name pg -e POSTGRES_USER=codspeed -e POSTGRES_PASSWORD=codspeed \
    ///     -e POSTGRES_DB=codspeed_bench -p 5546:5432 fargito/test-pg-tracer:16
    ///   psql "$DSN" -c "create table t(id int primary key, v text)"
    ///   PG_SMOKE_DSN="postgresql://codspeed:codspeed@127.0.0.1:5546/codspeed_bench?sslmode=disable" \
    ///     cargo test --lib instruments::postgres::tests::capture_isolates_benchmarks_against_real_db
    #[tokio::test]
    async fn capture_isolates_benchmarks_against_real_db() {
        let Ok(dsn) = std::env::var("PG_SMOKE_DSN") else {
            return;
        };
        let mut obs = PostgresInstrument::connect(&PostgresConfig {
            dsn_env_name: "PG_SMOKE_DSN".into(),
        })
        .await
        .unwrap();
        let (app, conn) = tokio_postgres::connect(&dsn, NoTls).await.unwrap();
        tokio::spawn(async move {
            let _ = conn.await;
        });
        app.simple_query("create table if not exists t(id int primary key, v text)")
            .await
            .unwrap();

        obs.reset().await.unwrap();
        for _ in 0..3 {
            app.execute("select count(*) from t where id = $1", &[&7i32])
                .await
                .unwrap();
        }
        obs.snapshot().await.unwrap();

        obs.reset().await.unwrap();
        for _ in 0..2 {
            app.execute("select count(*) from t where v = $1", &[&"v9"])
                .await
                .unwrap();
        }
        obs.snapshot().await.unwrap();

        let ts = ExecutionTimestamps {
            uri_by_ts: vec![(1, "bench::a".into()), (2, "bench::b".into())],
            markers: vec![],
        };
        let dir = std::env::temp_dir().join(format!("pg-smoke-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        obs.finalize(&ts, &dir).await.unwrap();

        let out = tokio::fs::read_to_string(dir.join("instruments/postgres.json"))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let b = v["benchmarks"].as_array().unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0]["uri"], "bench::a");
        assert_eq!(b[1]["uri"], "bench::b");
        let a_has_id_with_plan = b[0]["queries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|q| q["sql"].as_str().unwrap().contains("where id =") && !q["plan"].is_null());
        assert!(
            a_has_id_with_plan,
            "bench A must have the id query with a plan: {out}"
        );
        let b_leaked = b[1]["queries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|q| q["sql"].as_str().unwrap().contains("where id ="));
        assert!(
            !b_leaked,
            "reset failed: bench B leaked bench A's queries: {out}"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
