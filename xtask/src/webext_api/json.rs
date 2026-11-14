use serde_json::Value;

pub(super) fn parse(source: &str) -> Result<Value, String> {
    let source = strip_comments(source)?;
    let mut bytes = source.into_bytes();
    let mut quoted = false;
    let mut escaped = false;
    for index in 0..bytes.len() {
        let byte = bytes[index];
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b',' {
            let next = bytes[index + 1..]
                .iter()
                .find(|byte| !byte.is_ascii_whitespace());
            if matches!(next, Some(b']' | b'}')) {
                bytes[index] = b' ';
            }
        }
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

pub(super) fn strip_comments(source: &str) -> Result<String, String> {
    let mut bytes = source.as_bytes().to_vec();
    let mut index = 0;
    let mut quoted = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            while index < bytes.len() && bytes[index] != b'\n' {
                bytes[index] = b' ';
                index += 1;
            }
            continue;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            bytes[index] = b' ';
            bytes[index + 1] = b' ';
            index += 2;
            loop {
                if index + 1 >= bytes.len() {
                    return Err("unterminated block comment".into());
                }
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    bytes[index] = b' ';
                    bytes[index + 1] = b' ';
                    index += 2;
                    break;
                }
                if bytes[index] != b'\n' && bytes[index] != b'\r' {
                    bytes[index] = b' ';
                }
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comments_and_trailing_commas_leave_strings_intact() {
        assert_eq!(
            parse(
                r#"/* before */ {"url":"https://example/*x*/", // comment
                "quote":"\\\"//", "items":[1, /* tail */],}"#
            )
            .unwrap(),
            json!({"url":"https://example/*x*/", "quote":"\\\"//", "items":[1]})
        );
    }

    #[test]
    fn malformed_comments_and_json_fail() {
        assert!(parse("{/* missing").is_err());
        assert!(parse("{bad:1}").is_err());
    }
}
