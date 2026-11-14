//! Bounded extension resource names and descriptor-relative package reads.

use std::fmt;
use std::fs::File;
use std::io;
use std::path::Path;

use rustix::fs::{Mode, OFlags, ResolveFlags};

pub const MAX_URI_LENGTH: usize = 16 * 1024;
pub const MAX_RESOURCE_PATH_LENGTH: usize = 4095;
const MAX_COMPONENT_LENGTH: usize = 255;
const MAX_COMPONENTS: usize = 128;

#[derive(Debug)]
pub enum PackageError {
    Forbidden,
    NotFound,
    Io(io::Error),
}

impl PackageError {
    pub fn status_code(&self) -> u32 {
        match self {
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::Io(_) => 500,
        }
    }
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Forbidden => "extension resource is forbidden",
            Self::NotFound => "extension resource was not found",
            Self::Io(_) => "extension resource could not be read",
        })
    }
}

impl std::error::Error for PackageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// Parse the original request spelling before a URL parser can erase dot segments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionUri {
    host: String,
    resource: ResourcePath,
}

impl ExtensionUri {
    pub fn parse(uri: &str) -> Result<Self, PackageError> {
        if uri.len() > MAX_URI_LENGTH
            || uri
                .bytes()
                .any(|byte| byte <= b' ' || byte == 0x7f || byte == b'\\')
        {
            return Err(PackageError::Forbidden);
        }
        let rest = uri
            .strip_prefix("chrome-extension://")
            .ok_or(PackageError::Forbidden)?;
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let host = &rest[..authority_end];
        if host.len() != 32 || !host.bytes().all(|byte| (b'a'..=b'p').contains(&byte)) {
            return Err(PackageError::Forbidden);
        }
        let suffix = &rest[authority_end..];
        let path = if suffix.starts_with('/') {
            &suffix[..suffix.find(['?', '#']).unwrap_or(suffix.len())]
        } else {
            "/"
        };
        Ok(Self {
            host: host.to_owned(),
            resource: ResourcePath::parse(path)?,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn resource(&self) -> &ResourcePath {
        &self.resource
    }
}

/// A once-decoded, case-preserving relative name. It must never be decoded again.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ResourcePath(String);

impl ResourcePath {
    /// Accept only an absolute URI path, without its query or fragment.
    pub fn parse(path: &str) -> Result<Self, PackageError> {
        if path.len() > MAX_RESOURCE_PATH_LENGTH * 3 + 1 || path.contains(['?', '#']) {
            return Err(PackageError::Forbidden);
        }
        let encoded = path.strip_prefix('/').ok_or(PackageError::Forbidden)?;
        let bytes = encoded.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len().min(MAX_RESOURCE_PATH_LENGTH));
        let mut index = 0;
        while index < bytes.len() {
            let byte = if bytes[index] == b'%' {
                let high = bytes
                    .get(index + 1)
                    .and_then(|byte| hex_digit(*byte))
                    .ok_or(PackageError::Forbidden)?;
                let low = bytes
                    .get(index + 2)
                    .and_then(|byte| hex_digit(*byte))
                    .ok_or(PackageError::Forbidden)?;
                index += 3;
                let byte = high * 16 + low;
                if byte == b'/' || byte == b'\\' {
                    return Err(PackageError::Forbidden);
                }
                byte
            } else {
                let byte = bytes[index];
                index += 1;
                byte
            };
            if byte == b'\\' || byte.is_ascii_control() || decoded.len() == MAX_RESOURCE_PATH_LENGTH
            {
                return Err(PackageError::Forbidden);
            }
            decoded.push(byte);
        }
        let decoded = String::from_utf8(decoded).map_err(|_| PackageError::Forbidden)?;
        if decoded.chars().any(char::is_control) {
            return Err(PackageError::Forbidden);
        }
        if !decoded.is_empty() {
            let mut components = 0;
            for component in decoded.split('/') {
                components += 1;
                if component.is_empty()
                    || component == "."
                    || component == ".."
                    || component.len() > MAX_COMPONENT_LENGTH
                    || components > MAX_COMPONENTS
                {
                    return Err(PackageError::Forbidden);
                }
            }
        }
        Ok(Self(decoded))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_reserved(&self) -> bool {
        self.in_namespace("_zephium")
    }

    pub fn is_favicon(&self) -> bool {
        self.in_namespace("_favicon")
    }

    fn in_namespace(&self, namespace: &str) -> bool {
        self.0 == namespace
            || self
                .0
                .strip_prefix(namespace)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// A capability for one prepared publication, independent of later path renames.
/// The host must retire its registry entry before removing or disabling an install.
#[derive(Debug)]
pub struct PreparedPackage {
    directory: File,
}

impl PreparedPackage {
    pub fn open(directory: &Path) -> Result<Self, PackageError> {
        let descriptor = rustix::fs::openat2(
            rustix::fs::CWD,
            directory,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(map_open_error)?;
        Ok(Self {
            directory: File::from(descriptor),
        })
    }

    pub fn open_resource(&self, path: &ResourcePath) -> Result<PackageFile, PackageError> {
        if path.is_reserved() || path.is_favicon() || path.in_namespace("_metadata") {
            return Err(PackageError::Forbidden);
        }
        if path.as_str().is_empty() {
            return Err(PackageError::NotFound);
        }
        let descriptor = rustix::fs::openat2(
            &self.directory,
            path.as_str(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(map_open_error)?;
        let file = File::from(descriptor);
        let metadata = file.metadata().map_err(PackageError::Io)?;
        if metadata.is_dir() {
            return Err(PackageError::NotFound);
        }
        if !metadata.is_file() || metadata.len() > zephium_webext::archive::Limits::default().file {
            return Err(PackageError::Forbidden);
        }
        Ok(PackageFile {
            file,
            length: metadata.len(),
            mime_type: mime_type(path),
        })
    }
}

#[derive(Debug)]
pub struct PackageFile {
    pub file: File,
    pub length: u64,
    pub mime_type: &'static str,
}

fn map_open_error(error: rustix::io::Errno) -> PackageError {
    use rustix::io::Errno;
    match error {
        Errno::NOENT | Errno::NOTDIR => PackageError::NotFound,
        Errno::ACCESS
        | Errno::PERM
        | Errno::LOOP
        | Errno::XDEV
        | Errno::AGAIN
        | Errno::NAMETOOLONG => PackageError::Forbidden,
        _ => PackageError::Io(error.into()),
    }
}

/// Chromium's extension loader uses its well-known MIME map, without OS overrides.
pub fn mime_type(path: &ResourcePath) -> &'static str {
    let filename = path.as_str().rsplit('/').next().unwrap_or_default();
    let lowercase = filename.to_ascii_lowercase();
    let (stem, extension) = lowercase.rsplit_once('.').unwrap_or_default();
    let compound = stem.rsplit_once('.').is_some_and(|(_, previous)| {
        (previous == "user" && extension == "js")
            || (matches!(
                extension,
                "bz" | "bz2" | "gz" | "lz" | "lzma" | "lzo" | "xz" | "z" | "zst"
            ) && (1..=4).contains(&previous.len()))
    });
    if compound {
        return "application/octet-stream";
    }
    match extension {
        "js" | "mjs" => "text/javascript",
        "wasm" => "application/wasm",
        "css" => "text/css",
        "html" | "htm" | "shtml" | "shtm" | "ehtml" => "text/html",
        "xhtml" | "xht" | "xhtm" => "application/xhtml+xml",
        "json" => "application/json",
        "xml" | "xsl" | "xbl" | "xslt" => "text/xml",
        "txt" | "text" => "text/plain",
        "csv" => "text/csv",
        "md" => "text/markdown",
        "ics" => "text/calendar",
        "vtt" => "text/vtt",
        "sh" => "text/x-sh",
        "png" => "image/png",
        "apng" => "image/apng",
        "avif" => "image/avif",
        "jpg" | "jpeg" | "jpe" | "jfif" | "pjpeg" | "pjp" => "image/jpeg",
        "jxl" => "image/jxl",
        "gif" => "image/gif",
        "svg" | "svgz" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        "xbm" => "image/x-xbitmap",
        "woff" => "application/font-woff",
        "webm" => "video/webm",
        "mp4" | "m4v" => "video/mp4",
        "ogv" | "ogm" => "video/ogg",
        "mpeg" | "mpg" | "mpe" => "video/mpeg",
        "mkv" => "video/matroska",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "mka" => "audio/matroska",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/x-m4a",
        "weba" => "audio/webm",
        "m3u8" => "application/x-mpegurl",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "epub" => "application/epub+zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "crx" => "application/x-chrome-extension",
        "p7m" | "p7c" | "p7z" => "application/pkcs7-mime",
        "p7s" => "application/pkcs7-signature",
        "cer" | "crt" => "application/x-x509-ca-cert",
        "ps" | "eps" | "ai" => "application/postscript",
        "rdf" => "application/rdf+xml",
        "rss" => "application/rss+xml",
        "rtf" => "application/rtf",
        "apk" => "application/vnd.android.package-archive",
        "xul" => "application/vnd.mozilla.xul+xml",
        "doc" | "dot" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "swf" | "swl" => "application/x-shockwave-flash",
        "mht" | "mhtml" => "multipart/related",
        "eml" => "message/rfc822",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::fs::symlink;

    const ID: &str = "abcdefghijklmnopabcdefghijklmnop";

    fn path(path: &str) -> ResourcePath {
        ResourcePath::parse(path).unwrap()
    }

    #[test]
    fn preserves_case_unicode_and_single_decoding() {
        for (encoded, expected) in [
            ("/Scripts/Background.MJS", "Scripts/Background.MJS"),
            ("/space%20name.js", "space name.js"),
            ("/caf%C3%A9.js", "café.js"),
            ("/literal%252e%252e.js", "literal%2e%2e.js"),
            ("/literal%252f.js", "literal%2f.js"),
            ("/hash%23.js", "hash#.js"),
            ("/plus+name.js", "plus+name.js"),
        ] {
            assert_eq!(path(encoded).as_str(), expected);
        }
    }

    #[test]
    fn refuses_traversal_separators_controls_and_malformed_encoding() {
        for encoded in [
            "relative.js",
            "//absolute.js",
            "/../outside.js",
            "/./script.js",
            "/a/../script.js",
            "/%2e%2e/outside.js",
            "/.%2e/outside.js",
            "/%2e/script.js",
            "/a%2Fb.js",
            "/a%2fb.js",
            "/a%5Cb.js",
            "/a\\b.js",
            "/a//b.js",
            "/a/",
            "/a%00.js",
            "/a%0a.js",
            "/a\n.js",
            "/a%C2%85.js",
            "/%ff.js",
            "/%c0%ae%c0%ae/a.js",
            "/%.js",
            "/%0",
            "/%gg.js",
            "/a.js?query",
            "/a.js#fragment",
        ] {
            assert!(
                matches!(ResourcePath::parse(encoded), Err(PackageError::Forbidden)),
                "{encoded:?}"
            );
        }
    }

    #[test]
    fn bounds_components_decoded_paths_and_original_uri() {
        assert!(ResourcePath::parse(&format!("/{}", "a".repeat(255))).is_ok());
        assert!(ResourcePath::parse(&format!("/{}", "a".repeat(256))).is_err());
        assert!(ResourcePath::parse(&format!("/{}", vec!["a"; 128].join("/"))).is_ok());
        assert!(ResourcePath::parse(&format!("/{}", vec!["a"; 129].join("/"))).is_err());
        let maximum = vec!["a".repeat(255); 16].join("/");
        assert_eq!(maximum.len(), MAX_RESOURCE_PATH_LENGTH);
        assert!(ResourcePath::parse(&format!("/{maximum}")).is_ok());
        assert!(ResourcePath::parse(&format!("/{maximum}/a")).is_err());
        let uri = format!("chrome-extension://{ID}/a?{}", "x".repeat(MAX_URI_LENGTH));
        assert!(ExtensionUri::parse(&uri).is_err());
    }

    #[test]
    fn parses_the_raw_uri_without_normalizing_traversal() {
        let parsed = ExtensionUri::parse(&format!(
            "chrome-extension://{ID}/a%20b.MJS?q=../x#fragment"
        ))
        .unwrap();
        assert_eq!(parsed.host(), ID);
        assert_eq!(parsed.resource().as_str(), "a b.MJS");
        for tail in ["", "?query", "#fragment"] {
            assert_eq!(
                ExtensionUri::parse(&format!("chrome-extension://{ID}{tail}"))
                    .unwrap()
                    .resource()
                    .as_str(),
                ""
            );
        }
        for uri in [
            format!("chrome-extension://{ID}/a/../private.js"),
            format!("chrome-extension://{ID}/a/%2e%2e/private.js"),
            format!("chrome-extension://user@{ID}/script.js"),
            format!("chrome-extension://{ID}:80/script.js"),
            format!("chrome-extension://{}A/script.js", &ID[..31]),
            format!("chrome-extension://{}q/script.js", &ID[..31]),
            format!("chrome-extension://{}./script.js", ID),
            format!("chrome-extension://{ID}/script.js\n"),
            format!("https://{ID}/script.js"),
        ] {
            assert!(
                matches!(ExtensionUri::parse(&uri), Err(PackageError::Forbidden)),
                "{uri:?}"
            );
        }
    }

    #[test]
    fn namespaces_are_exact_and_cannot_be_hidden_with_encoding() {
        assert!(path("/%5fzephium/bootstrap.js").is_reserved());
        assert!(path("/_zephium").is_reserved());
        assert!(!path("/_zephium-other/script.js").is_reserved());
        assert!(!path("/other/_zephium/script.js").is_reserved());
        assert!(path("/_favicon/size/16").is_favicon());
    }

    #[test]
    fn mime_types_follow_chromium_without_sniffing_or_platform_overrides() {
        for (name, expected) in [
            ("/worker.js", "text/javascript"),
            ("/worker.MJS", "text/javascript"),
            ("/module.wasm", "application/wasm"),
            ("/page.html", "text/html"),
            ("/style.css", "text/css"),
            ("/data.json", "application/json"),
            ("/icon.svg", "image/svg+xml"),
            ("/icon.ico", "image/x-icon"),
            ("/font.woff", "application/font-woff"),
            ("/font.woff2", "application/octet-stream"),
            ("/archive.tar.gz", "application/octet-stream"),
            ("/archive.gz", "application/gzip"),
            ("/script.user.js", "application/octet-stream"),
            ("/unknown.extension", "application/octet-stream"),
            ("/no-extension", "application/octet-stream"),
            ("/scripts.js/unknown", "application/octet-stream"),
        ] {
            assert_eq!(mime_type(&path(name)), expected, "{name}");
        }
    }

    #[test]
    fn opens_only_exact_regular_resources_and_separates_refusal_from_missing() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("pkg-digest");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("Script.js"), b"export default 1").unwrap();
        std::fs::create_dir(root.join("directory")).unwrap();
        let package = PreparedPackage::open(&root).unwrap();
        let mut opened = package.open_resource(&path("/Script.js")).unwrap();
        assert_eq!(opened.mime_type, "text/javascript");
        let mut body = String::new();
        opened.file.read_to_string(&mut body).unwrap();
        assert_eq!(body, "export default 1");
        for missing in ["/script.js", "/missing.js", "/directory", "/"] {
            assert!(
                matches!(
                    package.open_resource(&path(missing)),
                    Err(PackageError::NotFound)
                ),
                "{missing}"
            );
        }
        for forbidden in [
            "/_zephium/background.html",
            "/_favicon/icon.png",
            "/_metadata/verified_contents.json",
        ] {
            assert!(
                matches!(
                    package.open_resource(&path(forbidden)),
                    Err(PackageError::Forbidden)
                ),
                "{forbidden}"
            );
        }
    }

    #[test]
    fn refuses_symlinks_at_the_root_and_every_resource_component() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("pkg-digest");
        let outside = temporary.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.js"), b"secret").unwrap();
        std::fs::write(root.join("inside.js"), b"inside").unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        symlink(outside.join("secret.js"), root.join("secret.js")).unwrap();
        symlink(root.join("inside.js"), root.join("local-link.js")).unwrap();
        symlink(&root, temporary.path().join("alias")).unwrap();
        assert!(PreparedPackage::open(&temporary.path().join("alias")).is_err());
        let package = PreparedPackage::open(&root).unwrap();
        for name in ["/escape/secret.js", "/secret.js", "/local-link.js"] {
            assert!(
                matches!(
                    package.open_resource(&path(name)),
                    Err(PackageError::Forbidden)
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn a_replaced_publication_path_cannot_retarget_its_open_descriptor() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("pkg-digest");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("script.js"), b"published").unwrap();
        let package = PreparedPackage::open(&root).unwrap();
        std::fs::rename(&root, temporary.path().join("retired")).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("script.js"), b"replacement").unwrap();
        let mut opened = package.open_resource(&path("/script.js")).unwrap();
        let mut body = String::new();
        opened.file.read_to_string(&mut body).unwrap();
        assert_eq!(body, "published");
    }

    #[test]
    fn symlink_replacement_races_cannot_serve_outside_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("pkg-digest");
        let outside = temporary.path().join("outside");
        std::fs::create_dir_all(root.join("child")).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(root.join("child/payload.js"), b"inside").unwrap();
        std::fs::write(outside.join("payload.js"), b"outside").unwrap();
        symlink(&outside, root.join("trap")).unwrap();
        let package = PreparedPackage::open(&root).unwrap();
        let start = std::sync::Arc::new(std::sync::Barrier::new(2));
        let attacker_start = start.clone();
        let attacker = std::thread::spawn(move || {
            attacker_start.wait();
            for _ in 0..1000 {
                rustix::fs::renameat_with(
                    rustix::fs::CWD,
                    root.join("child"),
                    rustix::fs::CWD,
                    root.join("trap"),
                    rustix::fs::RenameFlags::EXCHANGE,
                )
                .unwrap();
            }
        });
        start.wait();
        for _ in 0..1000 {
            match package.open_resource(&path("/child/payload.js")) {
                Ok(mut resource) => {
                    let mut bytes = Vec::new();
                    resource.file.read_to_end(&mut bytes).unwrap();
                    assert_eq!(bytes, b"inside");
                }
                Err(PackageError::Forbidden) => {}
                Err(error) => panic!("unexpected race result: {error}"),
            }
        }
        attacker.join().unwrap();
    }

    #[test]
    fn returns_a_stream_for_large_files_and_refuses_nonregular_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let large = temporary.path().join("large.wasm");
        let mut writer = File::create(&large).unwrap();
        writer.set_len(8 * 1024 * 1024).unwrap();
        writer.seek(SeekFrom::End(-4)).unwrap();
        writer.write_all(b"tail").unwrap();
        let fifo = temporary.path().join("pipe");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            Mode::RUSR | Mode::WUSR,
            0,
        )
        .unwrap();
        let package = PreparedPackage::open(temporary.path()).unwrap();
        let mut opened = package.open_resource(&path("/large.wasm")).unwrap();
        assert_eq!(opened.length, 8 * 1024 * 1024);
        opened.file.seek(SeekFrom::End(-4)).unwrap();
        let mut tail = [0; 4];
        opened.file.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, b"tail");
        assert!(matches!(
            package.open_resource(&path("/pipe")),
            Err(PackageError::Forbidden)
        ));
        writer
            .set_len(zephium_webext::archive::Limits::default().file + 1)
            .unwrap();
        assert!(matches!(
            package.open_resource(&path("/large.wasm")),
            Err(PackageError::Forbidden)
        ));
    }

    #[test]
    fn errors_expose_status_without_disclosing_filesystem_paths() {
        assert_eq!(PackageError::Forbidden.status_code(), 403);
        assert_eq!(PackageError::NotFound.status_code(), 404);
        let failure = PackageError::Io(io::Error::other("/private/extension/path"));
        assert_eq!(failure.status_code(), 500);
        assert!(!failure.to_string().contains("/private"));
    }
}
