use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};

use crate::run_environment::RepositoryProvider;

static REMOTE_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?P<domain>[^/@\.]+\.\w+)[:/](?P<owner>[^/]+)/(?P<repository>[^/]+?)(\.git)?/?$",
    )
    .unwrap()
});

#[derive(Debug)]
pub struct GitRemote {
    pub domain: String,
    pub owner: String,
    pub repository: String,
}

/// Parsed repository info including the CodSpeed provider
#[derive(Debug)]
pub struct ParsedRepository {
    pub provider: RepositoryProvider,
    pub owner: String,
    pub name: String,
}

/// Parse a git remote URL and extract the provider, owner, and repository name
pub fn parse_repository_from_remote(remote_url: &str) -> Result<ParsedRepository> {
    let GitRemote {
        domain,
        owner,
        repository,
    } = parse_git_remote(remote_url)?;
    let provider = match domain.as_str() {
        "github.com" => RepositoryProvider::GitHub,
        "gitlab.com" => RepositoryProvider::GitLab,
        domain => bail!("Repository provider {domain} is not supported by CodSpeed"),
    };

    Ok(ParsedRepository {
        provider,
        owner,
        name: repository,
    })
}

pub fn parse_git_remote(remote: &str) -> Result<GitRemote> {
    let captures = REMOTE_REGEX.captures(remote).ok_or_else(|| {
        anyhow!("Could not extract owner and repository from remote url: {remote}")
    })?;

    let domain = captures.name("domain").unwrap().as_str();
    let owner = captures.name("owner").unwrap().as_str();
    let repository = captures.name("repository").unwrap().as_str();

    Ok(GitRemote {
        domain: domain.to_string(),
        owner: owner.to_string(),
        repository: repository.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_remote() {
        let remote = "git@github.com:CodSpeedHQ/codspeed.git";
        let git_remote = parse_git_remote(remote).unwrap();
        insta::assert_debug_snapshot!(git_remote, @r###"
        GitRemote {
            domain: "github.com",
            owner: "CodSpeedHQ",
            repository: "codspeed",
        }
        "###);

        let remote = "https://github.com/CodSpeedHQ/codspeed.git";
        let git_remote = parse_git_remote(remote).unwrap();
        insta::assert_debug_snapshot!(git_remote, @r###"
        GitRemote {
            domain: "github.com",
            owner: "CodSpeedHQ",
            repository: "codspeed",
        }
        "###);

        let remote = "https://github.com/CodSpeedHQ/codspeed";
        let git_remote = parse_git_remote(remote).unwrap();
        insta::assert_debug_snapshot!(git_remote, @r###"
        GitRemote {
            domain: "github.com",
            owner: "CodSpeedHQ",
            repository: "codspeed",
        }
        "###);

        let remote = "git@gitlab.com:codspeed/runner.git";
        let git_remote = parse_git_remote(remote).unwrap();
        insta::assert_debug_snapshot!(git_remote, @r###"
        GitRemote {
            domain: "gitlab.com",
            owner: "codspeed",
            repository: "runner",
        }
        "###);

        let remote = "https://gitlab.com/codspeed/runner.git";
        let git_remote = parse_git_remote(remote).unwrap();
        insta::assert_debug_snapshot!(git_remote, @r###"
        GitRemote {
            domain: "gitlab.com",
            owner: "codspeed",
            repository: "runner",
        }
        "###);

        let remote = "https://github.com/codspeed/runner/";
        let git_remote = parse_git_remote(remote).unwrap();
        insta::assert_debug_snapshot!(git_remote, @r###"
        GitRemote {
            domain: "github.com",
            owner: "codspeed",
            repository: "runner",
        }
        "###);
    }

    #[test]
    fn test_parse_repository_from_remote() {
        use crate::run_environment::RepositoryProvider;

        let remote_urls = [
            (
                "git@github.com:CodSpeedHQ/codspeed.git",
                RepositoryProvider::GitHub,
                "CodSpeedHQ",
                "codspeed",
            ),
            (
                "https://github.com/CodSpeedHQ/codspeed.git",
                RepositoryProvider::GitHub,
                "CodSpeedHQ",
                "codspeed",
            ),
            (
                "git@gitlab.com:codspeed/runner.git",
                RepositoryProvider::GitLab,
                "codspeed",
                "runner",
            ),
            (
                "https://gitlab.com/codspeed/runner.git",
                RepositoryProvider::GitLab,
                "codspeed",
                "runner",
            ),
        ];
        for (remote_url, expected_provider, expected_owner, expected_name) in
            remote_urls.into_iter()
        {
            let parsed = parse_repository_from_remote(remote_url).unwrap();
            assert_eq!(parsed.provider, expected_provider);
            assert_eq!(parsed.owner, expected_owner);
            assert_eq!(parsed.name, expected_name);
        }
    }
}
