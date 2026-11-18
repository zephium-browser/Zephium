//! Reject strong WebKit imports newer than the application's deployment floor.

use std::path::Path;
use std::process::Command;

// Introduced versions from WebKit's WKError.h and WKWebsiteDataRecord.h.
const WEBKIT_IMPORTS: &[(&str, [u64; 3])] = &[
    ("_WKErrorDomain", [10, 10, 0]),
    ("_WKWebsiteDataTypeCookies", [10, 11, 0]),
    ("_WKWebsiteDataTypeDiskCache", [10, 11, 0]),
    ("_WKWebsiteDataTypeFetchCache", [10, 13, 4]),
    ("_WKWebsiteDataTypeFileSystem", [13, 0, 0]),
    ("_WKWebsiteDataTypeIndexedDBDatabases", [10, 11, 0]),
    ("_WKWebsiteDataTypeLocalStorage", [10, 11, 0]),
    ("_WKWebsiteDataTypeMemoryCache", [10, 11, 0]),
    ("_WKWebsiteDataTypeServiceWorkerRegistrations", [10, 13, 4]),
    ("_WKWebsiteDataTypeSessionStorage", [10, 11, 0]),
    ("_WKWebsiteDataTypeWebSQLDatabases", [10, 11, 0]),
];

pub(crate) fn run(arguments: &[String]) -> Result<(), String> {
    let [binary] = arguments else {
        return Err("usage: cargo xtask check-macos-imports <binary>".into());
    };
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../desktop/tauri.conf.json");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let minimum = config
        .pointer("/bundle/macOS/minimumSystemVersion")
        .and_then(serde_json::Value::as_str)
        .ok_or("desktop/tauri.conf.json has no macOS minimumSystemVersion")?;
    let output = Command::new("nm")
        .args(["-m", "-u", binary])
        .output()
        .map_err(|error| format!("cannot run nm: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "nm failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let output = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
    check(&output, version(minimum)?, WEBKIT_IMPORTS)?;
    println!("WebKit imports support macOS {minimum}: {binary}");
    Ok(())
}

fn version(value: &str) -> Result<[u64; 3], String> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 3 {
        return Err(format!("invalid macOS version: {value}"));
    }
    let mut version = [0; 3];
    for (part, slot) in parts.into_iter().zip(&mut version) {
        *slot = part
            .parse()
            .map_err(|_| format!("invalid macOS version: {value}"))?;
    }
    Ok(version)
}

#[derive(Debug, PartialEq)]
struct Import<'a> {
    symbol: &'a str,
    weak: bool,
}

fn parse(output: &str) -> Result<Vec<Import<'_>>, String> {
    output
        .lines()
        .filter_map(|line| line.split_once("(from WebKit)"))
        .map(|(prefix, _)| {
            let fields = prefix.split_whitespace().collect::<Vec<_>>();
            let symbol = fields
                .last()
                .filter(|symbol| symbol.starts_with('_'))
                .ok_or_else(|| format!("cannot parse WebKit import: {prefix}"))?;
            if !fields.contains(&"(undefined)") {
                return Err(format!("expected an undefined WebKit import: {prefix}"));
            }
            Ok(Import {
                symbol,
                weak: fields.windows(2).any(|pair| pair == ["weak", "external"]),
            })
        })
        .collect()
}

fn check(output: &str, minimum: [u64; 3], allowlist: &[(&str, [u64; 3])]) -> Result<(), String> {
    for (symbol, introduced) in allowlist {
        if *introduced > minimum {
            return Err(format!(
                "allowlisted {symbol} needs macOS {}.{}.{}, newer than minimumSystemVersion",
                introduced[0], introduced[1], introduced[2]
            ));
        }
    }
    for import in parse(output)? {
        if !import.weak && !allowlist.iter().any(|(symbol, _)| *symbol == import.symbol) {
            return Err(format!(
                "unreviewed strong WebKit import: {}",
                import.symbol
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
                 (undefined) external _WKErrorDomain (from WebKit)
                 (undefined) external _WKWebExtensionDataTypeLocal (from WebKit)
                 (undefined) weak external _WKFutureConstant (from WebKit)
                 (undefined) external _NSLog (from Foundation)
";

    #[test]
    fn parses_all_webkit_imports_including_weak_symbols() {
        assert_eq!(
            parse(SAMPLE).unwrap(),
            [
                Import {
                    symbol: "_WKErrorDomain",
                    weak: false
                },
                Import {
                    symbol: "_WKWebExtensionDataTypeLocal",
                    weak: false
                },
                Import {
                    symbol: "_WKFutureConstant",
                    weak: true
                },
            ]
        );
        assert!(parse("(undefined) external (from WebKit)").is_err());
    }

    #[test]
    fn accepts_reviewed_imports_and_unknown_weak_imports() {
        let output = "(undefined) external _WKErrorDomain (from WebKit)\n\
                      (undefined) weak external _WKFutureConstant (from WebKit)";
        assert!(check(output, [14, 0, 0], WEBKIT_IMPORTS).is_ok());
    }

    #[test]
    fn rejects_unknown_strong_imports() {
        let error = check(SAMPLE, [14, 0, 0], WEBKIT_IMPORTS).unwrap_err();
        assert!(error.contains("_WKWebExtensionDataTypeLocal"), "{error}");
    }

    #[test]
    fn rejects_allowlisted_versions_newer_than_the_configured_minimum() {
        let allowlist = [("_WKWebExtensionDataTypeLocal", [15, 4, 0])];
        let error = check("", version("14.0").unwrap(), &allowlist).unwrap_err();
        assert!(error.contains("15.4.0"), "{error}");
        assert!(check(
            SAMPLE,
            [15, 4, 0],
            &[
                ("_WKErrorDomain", [10, 10, 0]),
                ("_WKWebExtensionDataTypeLocal", [15, 4, 0]),
            ]
        )
        .is_ok());
    }

    #[test]
    fn parses_deployment_versions_and_rejects_malformed_values() {
        assert_eq!(version("14.0").unwrap(), [14, 0, 0]);
        assert_eq!(version("15.4.1").unwrap(), [15, 4, 1]);
        for invalid in ["", "14.", "fourteen", "14.0.0.1"] {
            assert!(version(invalid).is_err(), "{invalid}");
        }
    }
}
