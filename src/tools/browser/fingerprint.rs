//! Strip the obvious automation fingerprints from the driven browser.
//!
//! `--headless=new` cleaned up the user agent string, but the UA client
//! hints (`Sec-CH-UA` header and `navigator.userAgentData`) can still
//! advertise a `HeadlessChrome` brand, which anti-bot systems treat as
//! a signed confession. The overrides here make a headless session read
//! like the same browser headed: the UA string and client hints stay
//! consistent with the real binary, minus the headless marker.
//!
//! This changes only fingerprint-shaped values. The sandbox, the egress
//! policy and the cookie handling are untouched.

use serde_json::{json, Value};

/// A small script injected into every document before page scripts run.
/// Chrome only sets `navigator.webdriver` under automation, so a plain
/// `false` getter matches a human-driven browser exactly. Defined on
/// `Navigator.prototype`, where the real property lives.
pub const STEALTH_INIT_SCRIPT: &str = "\
try {
    Object.defineProperty(Navigator.prototype, 'webdriver', {
        get: () => false,
        configurable: true,
    });
} catch (e) {}";

/// Replace headless markers in a user agent string, keeping everything
/// else byte-identical (`HeadlessChrome/126.0` -> `Chrome/126.0`).
pub fn scrub_user_agent(user_agent: &str) -> String {
    user_agent.replace("HeadlessChrome", "Chrome")
}

/// Major version of the Chrome-family engine in a user agent string.
fn major_version(user_agent: &str) -> Option<String> {
    for token in ["Chrome/", "Chromium/", "Edg/"] {
        if let Some(rest) = user_agent.split(token).nth(1) {
            let major: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !major.is_empty() {
                return Some(major);
            }
        }
    }
    None
}

/// Full `Emulation.UserAgentMetadata` matching the given (already
/// scrubbed) user agent. Anti-bot systems cross-check the UA string
/// against these hints, so everything is derived from the real string
/// rather than invented. The few fields a UA string cannot express
/// (GPU-era platformVersion, kernel on Linux) are best-effort; they are
/// only readable through high-entropy client hints.
pub fn user_agent_metadata(scrubbed_user_agent: &str) -> Value {
    let ua = scrubbed_user_agent;
    let major = major_version(ua).unwrap_or_else(|| "126".to_string());
    let full_version = ["Chrome/", "Chromium/", "Edg/"]
        .iter()
        .find_map(|token| {
            ua.split(token)
                .nth(1)
                .map(|rest| rest.split(' ').next().unwrap_or(rest).to_string())
        })
        .unwrap_or_else(|| format!("{major}.0.0.0"));

    // Brand list mirrors Chrome's ordering: a grease brand, then the
    // engine, then the product name the UA string actually claims.
    let mut brands = vec![
        json!({"brand": "Not A;Brand", "version": "99"}),
        json!({"brand": "Chromium", "version": &major}),
    ];
    if ua.contains("Edg/") {
        brands.push(json!({"brand": "Microsoft Edge", "version": &major}));
    } else if ua.contains("Chrome/") {
        brands.push(json!({"brand": "Google Chrome", "version": &major}));
    }

    let (platform, platform_version, architecture, bitness) = platform_hints(ua);

    json!({
        "brands": brands,
        "fullVersion": full_version,
        "platform": platform,
        "platformVersion": platform_version,
        "architecture": architecture,
        "model": "",
        "mobile": false,
        "bitness": bitness,
        "wow64": false,
    })
}

fn platform_hints(ua: &str) -> (&'static str, String, &'static str, &'static str) {
    if ua.contains("Windows") {
        // Windows 11 still reports NT 10.0 in its UA string, so
        // "10.0.0" is the honest reading of it.
        return ("Windows", "10.0.0".to_string(), "x86", "64");
    }
    if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        // The macOS UA froze at 10_15_7 years ago; every real Chrome
        // on macOS sends it, so mirror it instead of guessing.
        let version = ua
            .split("Mac OS X ")
            .nth(1)
            .map(|rest| rest.split(';').next().unwrap_or(rest).replace('_', "."))
            .unwrap_or_else(|| "10.15.7".to_string());
        return ("macOS", version, "arm", "64");
    }
    if ua.contains("CrOS") {
        return ("Chrome OS", "6.6.0".to_string(), "x86", "64");
    }
    if ua.contains("Linux") || ua.contains("X11") {
        return ("Linux", "6.6.0".to_string(), "x86", "64");
    }
    ("Windows", "10.0.0".to_string(), "x86", "64")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_removes_headless_marker_only() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) HeadlessChrome/126.0.0.0 Safari/537.36";
        let scrubbed = scrub_user_agent(ua);
        assert!(!scrubbed.contains("Headless"));
        assert!(scrubbed.contains("Chrome/126.0.0.0"));
        assert!(scrubbed.starts_with("Mozilla/5.0 (Windows NT 10.0; Win64; x64)"));
    }

    #[test]
    fn metadata_matches_chrome_ua() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.6478.127 Safari/537.36";
        let meta = user_agent_metadata(ua);
        assert_eq!(meta["platform"], "Windows");
        assert_eq!(meta["architecture"], "x86");
        assert_eq!(meta["bitness"], "64");
        assert_eq!(meta["mobile"], false);
        assert_eq!(meta["fullVersion"], "126.0.6478.127");
        let brands: Vec<&str> = meta["brands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["brand"].as_str().unwrap())
            .collect();
        assert_eq!(brands, ["Not A;Brand", "Chromium", "Google Chrome"]);
        assert_eq!(meta["brands"][1]["version"], "126");
    }

    #[test]
    fn metadata_has_no_headless_brand() {
        let scrubbed = scrub_user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) HeadlessChrome/121.0.0.0 Safari/537.36",
        );
        let meta = user_agent_metadata(&scrubbed);
        assert!(!meta.to_string().contains("Headless"));
        assert_eq!(meta["platform"], "Linux");
    }

    #[test]
    fn init_script_patches_webdriver_to_false() {
        assert!(STEALTH_INIT_SCRIPT.contains("'webdriver'"));
        assert!(STEALTH_INIT_SCRIPT.contains("() => false"));
    }
}
