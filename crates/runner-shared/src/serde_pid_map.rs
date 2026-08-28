//! `#[serde(with = ...)]` support for pid-keyed maps.
//!
//! JSON object keys are always strings. serde_json's direct deserializer
//! special-cases that and parses integer map keys, but a `#[serde(flatten)]`
//! field is buffered into serde's internal `Content` first, and that path has no
//! such special case — an `i32` key then fails with `invalid type: string`. So
//! the keys are read as strings and parsed here, which works on both paths.

use libc::pid_t;
use serde::de::{Deserializer, Error};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;

pub fn serialize<V, S>(map: &HashMap<pid_t, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    V: Serialize,
    S: Serializer,
{
    map.serialize(serializer)
}

pub fn deserialize<'de, V, D>(deserializer: D) -> Result<HashMap<pid_t, V>, D::Error>
where
    V: Deserialize<'de>,
    D: Deserializer<'de>,
{
    HashMap::<String, V>::deserialize(deserializer)?
        .into_iter()
        .map(|(key, value)| {
            let pid = key
                .parse::<pid_t>()
                .map_err(|_| D::Error::custom(format!("invalid pid key: {key}")))?;
            Ok((pid, value))
        })
        .collect()
}
