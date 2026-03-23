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
