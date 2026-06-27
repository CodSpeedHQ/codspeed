use serde_json::json;

use super::UploadMetadata;

impl UploadMetadata {
    pub fn get_hash(&self) -> String {
        let upload_metadata_string = json!(&self).to_string();
        sha256::digest(upload_metadata_string)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use insta::{assert_json_snapshot, assert_snapshot};

    use crate::executor::ExecutorName;
    use crate::instruments::InstrumentName;
    use crate::run_environment::{
        GhData, LocalData, RepositoryProvider, RunEnvironment, RunEnvironmentMetadata, RunEvent,
        RunPart, Sender,
    };
    use crate::system::SystemInfo;
    use crate::upload::{LATEST_UPLOAD_METADATA_VERSION, Runner, UploadMetadata};

    #[test]
    fn test_get_metadata_hash() {
        let upload_metadata = UploadMetadata {
            repository_provider: RepositoryProvider::GitHub,
            version: Some(LATEST_UPLOAD_METADATA_VERSION),
            tokenless: true,
            profile_md5: "jp/k05RKuqP3ERQuIIvx4Q==".into(),
            profile_encoding: Some("gzip".into()),
            runner: Runner {
                name: "codspeed-runner".into(),
                version: "2.1.0".into(),
                instruments: vec![InstrumentName::MongoDB],
                executor: ExecutorName::Valgrind,
                system_info: SystemInfo::test(),
            },
            run_environment: RunEnvironment::GithubActions,
            commit_hash: "5bd77cb0da72bef094893ed45fb793ff16ecfbe3".into(),
            allow_empty: false,
            run_environment_metadata: RunEnvironmentMetadata {
                ref_: "refs/pull/29/merge".into(),
                head_ref: Some("chore/native-action-runner".into()),
                base_ref: Some("main".into()),
                owner: "CodSpeedHQ".into(),
                repository: "codspeed-node".into(),
                event: RunEvent::PullRequest,
                gh_data: Some(GhData {
                    run_id: "7044765741".into(),
                    job: "codspeed".into(),
                }),
                sender: Some(Sender {
                    id: "19605940".into(),
                    login: "adriencaccia".into(),
                }),
                gl_data: None,
                local_data: None,
                repository_root_path: "/home/runner/work/codspeed-node/codspeed-node/".into(),
            },
            run_part: Some(RunPart {
                run_id: "7044765741".into(),
                run_part_id: "benchmarks_3.2.2".into(),
                job_name: "codspeed".into(),
                metadata: BTreeMap::from([
                    ("someKey".into(), "someValue".into()),
                    ("anotherKey".into(), "anotherValue".into()),
                ]),
            }),
        };

        let hash = upload_metadata.get_hash();
        assert_snapshot!(
            hash,
            // Caution: when changing this value, we need to ensure that
            // the related backend snapshot remains the same
            @"0afc09ee58a610d400aa6b3fbdddf628608ed2e11aed39585a50abe96e1c9284"
        );
        assert_json_snapshot!(upload_metadata);
    }

    #[test]
    fn test_get_local_metadata_hash() {
        let upload_metadata = UploadMetadata {
            repository_provider: RepositoryProvider::Project,
            version: Some(LATEST_UPLOAD_METADATA_VERSION),
            tokenless: false,
            profile_md5: "tfC4VxYiYdJcTWpHpv4Ouw==".into(),
            profile_encoding: Some("gzip".into()),
            runner: Runner {
                name: "codspeed-runner".into(),
                version: "4.11.1".into(),
                instruments: vec![],
                executor: ExecutorName::Valgrind,
                system_info: SystemInfo {
                    os: crate::system::SupportedOs::Linux(
                        crate::system::LinuxDistribution::Other {
                            name: "nixos".into(),
                            version: "25.11".into(),
                        },
                    ),
                    arch: "x86_64".to_string(),
                    host: "badlands".to_string(),
                    user: "guillaume".to_string(),
                    cpu_brand: "11th Gen Intel(R) Core(TM) i5-11400H @ 2.70GHz".to_string(),
                    cpu_name: "cpu0".to_string(),
                    cpu_vendor_id: "GenuineIntel".to_string(),
                    cpu_cores: 6,
                    total_memory_gb: 16,
                    cpu_flags: vec![
                        "sse2".to_string(),
                        "avx".to_string(),
                        "avx2".to_string(),
                        "erms".to_string(),
                    ],
                },
            },
            run_environment: RunEnvironment::Local,
            commit_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            allow_empty: false,
            run_environment_metadata: RunEnvironmentMetadata {
                ref_: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                head_ref: None,
                base_ref: None,
                owner: "GuillaumeLagrange".into(),
                repository: "local-runs".into(),
                event: RunEvent::Local,
                gh_data: None,
                sender: None,
                gl_data: None,
                local_data: Some(LocalData {
                    expected_run_parts_count: 1,
                }),
                repository_root_path: "/home/guillaume/codspeed/runner/".into(),
            },
            run_part: Some(RunPart {
                run_id: "e0878123-c467-4191-994b-8560d8a7424e".into(),
                run_part_id: "valgrind".into(),
                job_name: "local-job".into(),
                metadata: BTreeMap::new(),
            }),
        };

        let hash = upload_metadata.get_hash();
        assert_snapshot!(
            hash,
            // Caution: when changing this value, we need to ensure that
            // the related backend snapshot remains the same
            @"26c83ef306f189fe5b725043577dbc09a204bbd1c973dd7d1e974ff88235dd84"
        );
        assert_json_snapshot!(upload_metadata);
    }
}
