use crate::limits::{MAX_CHARSET_BYTES, MAX_MEDIA_TYPE_BYTES, SNIFF_BYTES};
use crate::types::{Expect, ReadRefusal};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Declared {
    pub(crate) essence: String,
    pub(crate) charset: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Admission {
    /// The body is a gzip file to be unwrapped (`.xml.gz`), not a content coding.
    pub(crate) wrapped: bool,
    /// No declared type: the decoded bytes must identify themselves.
    pub(crate) sniffed: bool,
}

fn token(text: &str, extra: &[u8]) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || extra.contains(&byte))
}

/// `Ok(None)` when no type was declared; a declared type that does not parse
/// is a wrong type, never a reason to guess.
pub(crate) fn parse_content_type(value: Option<&[u8]>) -> Result<Option<Declared>, ReadRefusal> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = std::str::from_utf8(value).map_err(|_| ReadRefusal::WrongContentType)?;
    let mut parts = value.split(';');
    let essence = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let valid = essence.len() <= MAX_MEDIA_TYPE_BYTES
        && essence.split_once('/').is_some_and(|(kind, subtype)| {
            token(kind, b"!#$&^_.+-") && token(subtype, b"!#$&^_.+-")
        });
    if !valid {
        return Err(ReadRefusal::WrongContentType);
    }
    let charset = parts.find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let value = value.trim().trim_matches('"').to_ascii_lowercase();
        (value.len() <= MAX_CHARSET_BYTES && token(&value, b"-_.:+")).then_some(value)
    });
    Ok(Some(Declared { essence, charset }))
}

fn family(essence: &str) -> Option<(Expect, bool)> {
    match essence {
        "text/html" | "application/xhtml+xml" => Some((Expect::Html, false)),
        "application/xml" | "text/xml" => Some((Expect::Xml, false)),
        "application/gzip" | "application/x-gzip" => Some((Expect::Xml, true)),
        "text/plain" | "text/markdown" => Some((Expect::Text, false)),
        other if other.starts_with("application/") && other.ends_with("+xml") => {
            Some((Expect::Xml, false))
        }
        _ => None,
    }
}

pub(crate) fn admit(expect: Expect, declared: Option<&Declared>) -> Result<Admission, ReadRefusal> {
    match declared {
        None => Ok(Admission {
            wrapped: false,
            sniffed: true,
        }),
        Some(declared) => match family(&declared.essence) {
            Some((family, wrapped)) if family == expect => Ok(Admission {
                wrapped,
                sniffed: wrapped,
            }),
            _ => Err(ReadRefusal::WrongContentType),
        },
    }
}

fn trimmed(head: &[u8]) -> &[u8] {
    let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let start = head
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(head.len());
    &head[start..]
}

fn starts_with_tag(head: &[u8], tags: &[&[u8]], self_closing: bool) -> bool {
    tags.iter().any(|tag| {
        head.len() >= tag.len()
            && head[..tag.len()].eq_ignore_ascii_case(tag)
            && head.get(tag.len()).is_none_or(|next| {
                next.is_ascii_whitespace()
                    || *next == b'>'
                    || self_closing && head[tag.len()..].starts_with(b"/>")
            })
    })
}

pub(crate) fn looks_html(head: &[u8]) -> bool {
    starts_with_tag(
        trimmed(head),
        &[
            b"<!doctype html",
            b"<html",
            b"<head",
            b"<body",
            b"<title",
            b"<script",
            b"<meta",
        ],
        false,
    )
}

pub(crate) fn looks_xml(head: &[u8]) -> bool {
    let head = trimmed(head);
    head.starts_with(b"<?xml")
        || starts_with_tag(
            head,
            &[b"<urlset", b"<sitemapindex", b"<rss", b"<feed"],
            true,
        )
}

pub(crate) fn looks_binary(head: &[u8]) -> bool {
    const MAGIC: [&[u8]; 14] = [
        b"\x89PNG",
        b"GIF87a",
        b"GIF89a",
        b"\xff\xd8\xff",
        b"%PDF-",
        b"PK\x03\x04",
        b"\x1f\x8b",
        b"RIFF",
        b"\x00asm",
        b"\x7fELF",
        b"7z\xbc\xaf",
        b"Rar!",
        b"\xfd7zXZ",
        b"OggS",
    ];
    let utf16 = head.starts_with(b"\xff\xfe") || head.starts_with(b"\xfe\xff");
    MAGIC.iter().any(|magic| head.starts_with(magic)) || !utf16 && head.contains(&0)
}

/// Checks the first decoded bytes against what was asked for.
pub(crate) fn check_body(
    expect: Expect,
    admission: Admission,
    decoded: &[u8],
) -> Result<(), ReadRefusal> {
    let head = &decoded[..decoded.len().min(SNIFF_BYTES)];
    if head.is_empty() {
        return if admission.sniffed {
            Err(ReadRefusal::WrongContentType)
        } else {
            Ok(())
        };
    }
    let refused = looks_binary(head)
        || match expect {
            Expect::Html => admission.sniffed && !looks_html(head),
            Expect::Xml => looks_html(head) || admission.sniffed && !looks_xml(head),
            Expect::Text => looks_html(head) || admission.sniffed && looks_xml(head),
        };
    if refused {
        Err(ReadRefusal::WrongContentType)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn declared(value: &str) -> Option<Declared> {
        parse_content_type(Some(value.as_bytes())).unwrap()
    }

    #[test]
    fn declared_types_parse_to_a_bounded_essence_and_charset() {
        assert_eq!(
            declared("Text/HTML; Charset=\"UTF-8\""),
            Some(Declared {
                essence: "text/html".into(),
                charset: Some("utf-8".into())
            })
        );
        assert_eq!(declared("text/plain").unwrap().charset, None);
        assert_eq!(declared("text/plain; charset=a b").unwrap().charset, None);
        let long_charset = format!("text/plain; charset={}", "a".repeat(MAX_CHARSET_BYTES + 1));
        assert_eq!(declared(&long_charset).unwrap().charset, None);
        let at_limit = format!("text/plain; charset={}", "a".repeat(MAX_CHARSET_BYTES));
        assert!(declared(&at_limit).unwrap().charset.is_some());
        assert_eq!(parse_content_type(None), Ok(None));
        for bad in ["", "html", "text/", "/html", "text/ html", "text/html/x"] {
            assert_eq!(
                parse_content_type(Some(bad.as_bytes())),
                Err(ReadRefusal::WrongContentType),
                "{bad}"
            );
        }
        let long = format!("text/{}", "a".repeat(MAX_MEDIA_TYPE_BYTES));
        assert_eq!(
            parse_content_type(Some(long.as_bytes())),
            Err(ReadRefusal::WrongContentType)
        );
        assert_eq!(
            parse_content_type(Some(b"text/\xffhtml")),
            Err(ReadRefusal::WrongContentType)
        );
    }

    #[test]
    fn each_family_accepts_only_its_own_types() {
        for (value, expect, wrapped) in [
            ("text/html", Expect::Html, false),
            ("application/xhtml+xml", Expect::Html, false),
            ("application/xml", Expect::Xml, false),
            ("text/xml", Expect::Xml, false),
            ("application/rss+xml", Expect::Xml, false),
            ("application/gzip", Expect::Xml, true),
            ("application/x-gzip", Expect::Xml, true),
            ("text/plain", Expect::Text, false),
            ("text/markdown", Expect::Text, false),
        ] {
            let declared = declared(value);
            assert_eq!(
                admit(expect, declared.as_ref()),
                Ok(Admission {
                    wrapped,
                    sniffed: wrapped
                })
            );
            for other in [Expect::Html, Expect::Xml, Expect::Text] {
                if other != expect {
                    assert_eq!(
                        admit(other, declared.as_ref()),
                        Err(ReadRefusal::WrongContentType)
                    );
                }
            }
        }
        for refused in [
            "application/octet-stream",
            "image/png",
            "application/json",
            "text/css",
        ] {
            for expect in [Expect::Html, Expect::Xml, Expect::Text] {
                assert_eq!(
                    admit(expect, declared(refused).as_ref()),
                    Err(ReadRefusal::WrongContentType)
                );
            }
        }
    }

    #[test]
    fn sniffing_stays_within_the_family_asked_for() {
        let sniffed = Admission {
            wrapped: false,
            sniffed: true,
        };
        let declared = Admission {
            wrapped: false,
            sniffed: false,
        };
        let html = b"\xef\xbb\xbf\n  <!DOCTYPE html><html><body>x</body></html>";
        let xml = b"<?xml version=\"1.0\"?><urlset/>";
        let text = b"User-agent: *\nDisallow:\n";
        assert_eq!(check_body(Expect::Html, sniffed, html), Ok(()));
        assert_eq!(
            check_body(Expect::Html, sniffed, text),
            Err(ReadRefusal::WrongContentType)
        );
        assert_eq!(check_body(Expect::Xml, sniffed, xml), Ok(()));
        assert_eq!(
            check_body(Expect::Xml, sniffed, b"<urlset xmlns=\"x\">"),
            Ok(())
        );
        assert_eq!(
            check_body(Expect::Xml, sniffed, text),
            Err(ReadRefusal::WrongContentType)
        );
        assert_eq!(check_body(Expect::Text, sniffed, text), Ok(()));
        assert_eq!(
            check_body(Expect::Text, sniffed, xml),
            Err(ReadRefusal::WrongContentType)
        );
        assert_eq!(
            check_body(Expect::Text, sniffed, b""),
            Err(ReadRefusal::WrongContentType)
        );
        assert_eq!(check_body(Expect::Text, declared, b""), Ok(()));
        assert_eq!(
            check_body(Expect::Text, declared, html),
            Err(ReadRefusal::WrongContentType)
        );
        assert_eq!(
            check_body(Expect::Xml, declared, html),
            Err(ReadRefusal::WrongContentType)
        );
        assert_eq!(check_body(Expect::Html, declared, b"plain words"), Ok(()));
        for binary in [
            &b"\x89PNG\r\n\x1a\n...."[..],
            b"%PDF-1.7",
            b"GIF89a",
            b"PK\x03\x04",
            b"<html>\0</html>",
        ] {
            for expect in [Expect::Html, Expect::Xml, Expect::Text] {
                assert_eq!(
                    check_body(expect, declared, binary),
                    Err(ReadRefusal::WrongContentType)
                );
            }
        }
        assert_eq!(
            check_body(Expect::Text, declared, b"\xff\xfeh\0i\0"),
            Ok(())
        );
        assert!(!looks_html(b"<htmlx>"));
        assert!(looks_html(b"<html"));
    }

    proptest! {
        #[test]
        fn header_and_body_checks_never_panic(value in proptest::collection::vec(any::<u8>(), 0..300), body in proptest::collection::vec(any::<u8>(), 0..700)) {
            if let Ok(Some(declared)) = parse_content_type(Some(&value)) {
                prop_assert!(declared.essence.len() <= MAX_MEDIA_TYPE_BYTES);
                prop_assert!(declared.charset.as_ref().is_none_or(|c| c.len() <= MAX_CHARSET_BYTES));
            }
            for expect in [Expect::Html, Expect::Xml, Expect::Text] {
                for sniffed in [false, true] {
                    let _ = check_body(expect, Admission { wrapped: false, sniffed }, &body);
                }
            }
        }
    }
}

#[cfg(test)]
#[test]
fn known_xml_roots_allow_immediate_self_closing_delimiters() {
    let admission = admit(Expect::Xml, None).unwrap();
    for body in [b"<rss/>".as_slice(), b"<rss />"] {
        assert_eq!(check_body(Expect::Xml, admission, body), Ok(()));
        assert!(check_body(Expect::Text, admit(Expect::Text, None).unwrap(), body).is_err());
    }
    assert!(check_body(Expect::Xml, admission, b"<rss-extra/>").is_err());
}
