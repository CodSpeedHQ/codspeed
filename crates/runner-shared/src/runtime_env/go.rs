const GODEBUG: &str = "GODEBUG";
/// Starts the global `math/rand` source from `Seed(1)` instead of a random seed.
const RANDAUTOSEED_OFF: &str = "randautoseed=0";

/// Go env:
/// - `GODEBUG=randautoseed=0`: deterministic global `math/rand` in every mode.
pub(super) fn env() -> Vec<(&'static str, String)> {
    let user_godebug = std::env::var(GODEBUG).ok();
    vec![(GODEBUG, godebug(user_godebug.as_deref()))]
}

/// Prepends our settings to the user's `GODEBUG`. Go uses the last occurrence
/// of a setting, so an explicit `randautoseed` from the user still wins.
fn godebug(user_godebug: Option<&str>) -> String {
    match user_godebug {
        Some(user) if !user.is_empty() => format!("{RANDAUTOSEED_OFF},{user}"),
        _ => RANDAUTOSEED_OFF.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_user_settings_after_ours() {
        assert_eq!(godebug(None), "randautoseed=0");
        assert_eq!(godebug(Some("")), "randautoseed=0");
        assert_eq!(
            godebug(Some("gctrace=1,randautoseed=1")),
            "randautoseed=0,gctrace=1,randautoseed=1"
        );
    }
}
