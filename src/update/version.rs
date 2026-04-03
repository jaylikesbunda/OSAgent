use semver::Version;
use std::cmp::Ordering;

pub fn parse_version(version_str: &str) -> Option<Version> {
    let normalized = normalize_version(version_str);
    Version::parse(&normalized).ok()
}

fn normalize_version(version_str: &str) -> String {
    let trimmed = version_str.trim();
    if trimmed.starts_with('v') {
        trimmed[1..].to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let version_a = parse_version(a)?;
    let version_b = parse_version(b)?;
    Some(version_a.cmp(&version_b))
}

pub fn is_newer(new_version: &str, current_version: &str) -> bool {
    compare_versions(new_version, current_version)
        .map(|ord| ord == Ordering::Greater)
        .unwrap_or(false)
}

/// Returns true if `candidate` is a prerelease (rc/alpha/beta) whose base version
/// matches or exceeds `current`. Used to surface RCs on the beta channel even when
/// the base version hasn't been released yet (e.g. 0.1.0-rc1 available while on 0.1.0).
pub fn is_prerelease_of(candidate: &str, current: &str) -> bool {
    let Some(candidate_v) = parse_version(candidate) else {
        return false;
    };
    if candidate_v.pre.is_empty() {
        return false;
    }
    let base_candidate = Version::new(candidate_v.major, candidate_v.minor, candidate_v.patch);
    let Some(current_v) = parse_version(current) else {
        return false;
    };
    let base_current = Version::new(current_v.major, current_v.minor, current_v.patch);
    base_candidate >= base_current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert!(parse_version("1.0.0").is_some());
        assert!(parse_version("v1.0.0").is_some());
        assert!(parse_version("0.1.0").is_some());
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Some(Ordering::Equal));
        assert_eq!(compare_versions("1.1.0", "1.0.0"), Some(Ordering::Greater));
        assert_eq!(compare_versions("1.0.0", "1.1.0"), Some(Ordering::Less));
    }

    #[test]
    fn test_is_newer() {
        assert!(is_newer("1.1.0", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.1.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
    }
}
