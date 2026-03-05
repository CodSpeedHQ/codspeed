use std::collections::HashSet;

use log::warn;
use serde::{Deserialize, Serialize};

use crate::cli::run::RunArgs;
use crate::prelude::*;

pub mod mongo_tracer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MongoDBConfig {
    pub uri_env_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruments {
    pub mongodb: Option<MongoDBConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub enum InstrumentName {
    MongoDB,
}

impl Instruments {
    pub fn is_mongodb_enabled(&self) -> bool {
        self.mongodb.is_some()
    }

    pub fn get_active_instrument_names(&self) -> Vec<InstrumentName> {
        let mut names = vec![];

        if self.is_mongodb_enabled() {
            names.push(InstrumentName::MongoDB);
        }

        names
    }
}

impl TryFrom<&RunArgs> for Instruments {
    type Error = Error;
    fn try_from(args: &RunArgs) -> Result<Self> {
        let mut validated_instrument_names: HashSet<InstrumentName> = HashSet::new();

        for instrument_name in &args.instruments {
            match instrument_name.as_str() {
                "mongodb" => validated_instrument_names.insert(InstrumentName::MongoDB),
                _ => bail!("Invalid instrument name: {instrument_name}"),
            };
        }

        let mongodb = if validated_instrument_names.contains(&InstrumentName::MongoDB) {
            Some(MongoDBConfig {
                uri_env_name: args.mongo_uri_env_name.clone(),
            })
        } else if args.mongo_uri_env_name.is_some() {
            warn!(
                "The MongoDB instrument is disabled but a MongoDB URI environment variable name was provided, ignoring it"
            );
            None
        } else {
            None
        };

        Ok(Self { mongodb })
    }
}

#[cfg(test)]
impl Instruments {
    /// Constructs a new `Instruments` with default values for testing purposes
    pub fn test() -> Self {
        Self {
            mongodb: Some(MongoDBConfig {
                uri_env_name: Some("MONGODB_URI".into()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_args_empty() {
        let instruments = Instruments::try_from(&RunArgs::test()).unwrap();
        assert!(instruments.mongodb.is_none());
    }

    #[test]
    fn test_from_args() {
        let args = RunArgs {
            instruments: vec!["mongodb".into()],
            mongo_uri_env_name: Some("MONGODB_URI".into()),
            ..RunArgs::test()
        };
        let instruments = Instruments::try_from(&args).unwrap();
        assert_eq!(
            instruments.mongodb,
            Some(MongoDBConfig {
                uri_env_name: Some("MONGODB_URI".into())
            })
        );
        assert!(instruments.is_mongodb_enabled());
    }

    #[test]
    fn test_from_args_mongodb_disabled() {
        let args = RunArgs {
            instruments: vec![],
            mongo_uri_env_name: Some("MONGODB_URI".into()),
            ..RunArgs::test()
        };
        let instruments = Instruments::try_from(&args).unwrap();
        assert_eq!(instruments.mongodb, None);
        assert!(!instruments.is_mongodb_enabled());
    }

    #[test]
    fn test_from_args_unknown_instrument_value() {
        let args = RunArgs {
            instruments: vec!["unknown".into()],
            mongo_uri_env_name: Some("MONGODB_URI".into()),
            ..RunArgs::test()
        };
        let instruments = Instruments::try_from(&args);
        assert!(instruments.is_err());
        assert_eq!(
            instruments.unwrap_err().to_string(),
            "Invalid instrument name: unknown"
        );
    }
}
