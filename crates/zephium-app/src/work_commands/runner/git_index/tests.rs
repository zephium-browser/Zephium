use super::*;
use std::{fs, io::Write};
fn parse_index(bytes: &[u8]) -> Result<index::Index, Error> {
    index::parse(bytes, &|| Ok(()))
}
fn index_bytes(entries: &[(&str, Oid, u32)]) -> Vec<u8> {
    let mut entries = entries.to_vec();
    entries.sort_by_key(|row| row.0);
    let mut out = b"DIRC".to_vec();
    out.extend(2u32.to_be_bytes());
    out.extend((entries.len() as u32).to_be_bytes());
    for (name, id, mode) in entries {
        let start = out.len();
        let mut fixed = [0; 40];
        fixed[24..28].copy_from_slice(&mode.to_be_bytes());
        out.extend(fixed);
        out.extend(id);
        out.extend((name.len().min(0xfff) as u16).to_be_bytes());
        out.extend(name.as_bytes());
        out.push(0);
        while !(out.len() - start).is_multiple_of(8) {
            out.push(0);
        }
    }
    out.extend(hash(&out));
    out
}
fn rechecksum(bytes: &mut [u8]) {
    let end = bytes.len() - 20;
    let digest = hash(&bytes[..end]);
    bytes[end..].copy_from_slice(&digest);
}
fn compressed(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
enum Encoding {
    Blob(Vec<u8>),
    Reference(Oid, Vec<u8>),
    Offset(usize, Vec<u8>),
    Raw(Vec<u8>),
}
fn header(kind: u8, mut size: usize) -> Vec<u8> {
    let mut out = vec![(kind << 4) | (size as u8 & 15)];
    size >>= 4;
    while size != 0 {
        *out.last_mut().unwrap() |= 128;
        out.push(size as u8 & 127);
        size >>= 7;
    }
    out
}
fn pack_bytes(objects: Vec<(Oid, Encoding)>) -> (Vec<u8>, Vec<u8>, Oid) {
    let mut data = b"PACK".to_vec();
    data.extend(2u32.to_be_bytes());
    data.extend((objects.len() as u32).to_be_bytes());
    let mut positions = Vec::new();
    let mut rows = Vec::new();
    for (id, encoding) in objects {
        let position = data.len();
        let entry = match encoding {
            Encoding::Blob(bytes) => {
                let mut out = header(3, bytes.len());
                out.extend(compressed(&bytes));
                out
            }
            Encoding::Reference(base, bytes) => {
                let mut out = header(7, bytes.len());
                out.extend(base);
                out.extend(compressed(&bytes));
                out
            }
            Encoding::Offset(base, bytes) => {
                let mut distance = position - positions[base];
                let mut encoded = vec![distance as u8 & 127];
                loop {
                    distance >>= 7;
                    if distance == 0 {
                        break;
                    }
                    distance -= 1;
                    encoded.push(128 | (distance as u8 & 127));
                }
                encoded.reverse();
                let mut out = header(6, bytes.len());
                out.extend(encoded);
                out.extend(compressed(&bytes));
                out
            }
            Encoding::Raw(bytes) => bytes,
        };
        positions.push(position);
        let mut crc = flate2::Crc::new();
        crc.update(&entry);
        rows.push((id, position as u32, crc.sum()));
        data.extend(entry);
    }
    let id = hash(&data);
    data.extend(id);
    rows.sort_by_key(|row| row.0);
    let mut index = b"\xfftOc".to_vec();
    index.extend(2u32.to_be_bytes());
    for bucket in 0..256 {
        let count = rows
            .iter()
            .filter(|row| usize::from(row.0[0]) <= bucket)
            .count() as u32;
        index.extend(count.to_be_bytes());
    }
    for row in &rows {
        index.extend(row.0);
    }
    for row in &rows {
        index.extend(row.2.to_be_bytes());
    }
    for row in &rows {
        index.extend(row.1.to_be_bytes());
    }
    index.extend(id);
    index.extend(hash(&index));
    (index, data, id)
}
fn delta() -> Vec<u8> {
    let mut data = vec![12, 11, 0x90, 6, 5];
    data.extend(b"rust\n");
    data
}
struct Fixture {
    _scratch: tempfile::TempDir,
    root: PathBuf,
    grant: WorkFileGrant,
}
impl Fixture {
    fn new() -> Self {
        let scratch = crate::work_files::test_tempdir();
        let home = scratch.path().canonicalize().unwrap();
        let root = home.join("code/repo");
        fs::create_dir_all(root.join(".git/objects/pack")).unwrap();
        let _home = crate::work_files::TestGrantHome::new(home);
        let (grant, refused) = WorkFileGrant::admit(&[root.to_string_lossy().into_owned()]);
        assert!(refused.is_empty());
        Self {
            _scratch: scratch,
            root,
            grant,
        }
    }
    fn index(&self, entries: &[(&str, Oid, u32)]) {
        fs::write(self.root.join(".git/index"), index_bytes(entries)).unwrap();
    }
    fn pack(&self, objects: Vec<(Oid, Encoding)>) {
        let (index, data, id) = pack_bytes(objects);
        let id = hex(&id);
        let base = self.root.join(format!(".git/objects/pack/pack-{id}"));
        fs::write(base.with_extension("idx"), index).unwrap();
        fs::write(base.with_extension("pack"), data).unwrap();
    }
    fn loose(&self, bytes: &[u8]) -> Oid {
        let id = blob_hash(bytes);
        let name = hex(&id);
        let parent = self.root.join(".git/objects").join(&name[..2]);
        fs::create_dir_all(&parent).unwrap();
        let mut body = format!("blob {}\0", bytes.len()).into_bytes();
        body.extend(bytes);
        fs::write(parent.join(&name[2..]), compressed(&body)).unwrap();
        id
    }
    fn capture(&self, paths: &[&str]) -> Result<Captured, Error> {
        capture(
            &self.grant,
            &self.root,
            &paths.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
            Arc::new(AtomicBool::new(false)),
        )
    }
}
fn hex(id: &Oid) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn index_filters_protected_oids_and_rejects_bad_shapes() {
    let id = blob_hash(b"inert");
    let raw = index_bytes(&[
        (".env", id, 0o100644),
        ("README.md", id, 0o100644),
        ("credentialſ.json", [2; 20], 0o100644),
        ("KEYCHA~1/file.txt", [3; 20], 0o100644),
    ]);
    let index = parse_index(&raw).unwrap();
    assert_eq!(index.entries.len(), 1);
    assert!(
        index.protected.contains(&id)
            && index.protected.contains(&[2; 20])
            && index.protected.contains(&[3; 20])
    );
    for size in 0..raw.len() {
        assert!(parse_index(&raw[..size]).is_err());
    }
    let mut corrupt = raw.clone();
    corrupt[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    rechecksum(&mut corrupt);
    assert!(matches!(parse_index(&corrupt), Err(Error::Limit)));
    let mut version = raw.clone();
    version[4..8].copy_from_slice(&4u32.to_be_bytes());
    rechecksum(&mut version);
    assert!(matches!(parse_index(&version), Err(Error::Unsupported)));
    for name in ["../outside", "/absolute", "a//b", "a/./b", "a\\b"] {
        assert!(parse_index(&index_bytes(&[(name, id, 0o100644)])).is_err());
    }
    assert!(parse_index(&index_bytes(&[
        ("a", id, 0o100644),
        ("a.b", id, 0o100644),
        ("a/x", id, 0o100644)
    ]))
    .is_err());
    let mut split = index_bytes(&[("a", id, 0o100644)]);
    split.truncate(split.len() - 20);
    split.extend(b"link");
    split.extend(0u32.to_be_bytes());
    split.extend(hash(&split));
    assert!(matches!(parse_index(&split), Err(Error::Unsupported)));
    let sparse = index_bytes(&[("tree", id, 0o040000)]);
    assert!(matches!(parse_index(&sparse), Err(Error::Unsupported)));
}
#[test]
fn pack_verifies_offsets_checksums_and_both_delta_encodings() {
    let base = b"hello world\n";
    let target = b"hello rust\n";
    let base_id = blob_hash(base);
    let target_id = blob_hash(target);
    for encoding in [
        Encoding::Reference(base_id, delta()),
        Encoding::Offset(0, delta()),
    ] {
        let (index, data, id) = pack_bytes(vec![
            (base_id, Encoding::Blob(base.to_vec())),
            (target_id, encoding),
        ]);
        let pack = pack::Pack::new(&index, data.clone(), id).unwrap();
        let mut budget = Budget::new();
        let pack::Payload::Delta {
            base: base_ref,
            bytes,
        } = pack.payload(&target_id, &mut budget).unwrap()
        else {
            panic!("delta expected")
        };
        assert_eq!(base_ref, base_id);
        assert_eq!(
            pack::apply_delta(base, &bytes, &mut budget).unwrap(),
            target
        );
        for size in 0..index.len() {
            assert!(pack::Pack::new(&index[..size], data.clone(), id).is_err());
        }
        for size in 0..data.len() {
            assert!(pack::Pack::new(&index, data[..size].to_vec(), id).is_err());
        }
        let mut offsets = index.clone();
        let at = 1032 + 2 * 20 + 2 * 4;
        offsets[at..at + 4].copy_from_slice(&0xffff_ffffu32.to_be_bytes());
        rechecksum(&mut offsets);
        assert!(pack::Pack::new(&offsets, data.clone(), id).is_err());
        let mut fanout = index.clone();
        fanout[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        rechecksum(&mut fanout);
        assert!(pack::Pack::new(&fanout, data.clone(), id).is_err());
        let mut bad_crc = index.clone();
        bad_crc[1032 + 2 * 20] ^= 1;
        rechecksum(&mut bad_crc);
        let changed = pack::Pack::new(&bad_crc, data, id).unwrap();
        assert!(changed
            .payload(
                &[base_id, target_id].into_iter().min().unwrap(),
                &mut Budget::new()
            )
            .is_err());
    }
    let (index, data, id) = pack_bytes(vec![([1; 20], Encoding::Raw(vec![0xbf; 16]))]);
    let pack = pack::Pack::new(&index, data, id).unwrap();
    assert!(pack.payload(&[1; 20], &mut Budget::new()).is_err());
}
#[test]
fn delta_programs_cannot_overflow_or_exceed_output() {
    for data in [
        vec![0xff; 20],
        vec![1, 1, 0],
        vec![1, 2, 0x90, 3],
        vec![1, 1, 0x91, 2, 1],
        vec![1, 0, 1, b'x'],
    ] {
        assert!(pack::apply_delta(b"x", &data, &mut Budget::new()).is_err());
    }
    let mut budget = Budget::new();
    budget.output(MAX_INFLATED).unwrap();
    assert_eq!(budget.output(1), Err(Error::Limit));
}
#[test]
fn capture_handles_loose_packed_and_deleted_files_and_detects_changes() {
    let fixture = Fixture::new();
    let old = fixture.loose(b"before\n");
    fixture.index(&[("note.txt", old, 0o100644)]);
    fs::write(fixture.root.join("note.txt"), b"after\n").unwrap();
    let captured = fixture.capture(&["note.txt"]).unwrap();
    assert_eq!(captured.before[Path::new("note.txt")], b"before\n");
    captured.unchanged(&AtomicBool::new(false)).unwrap();
    fs::write(fixture.root.join("note.txt"), b"another\n").unwrap();
    assert!(matches!(
        captured.unchanged(&AtomicBool::new(false)),
        Err(Error::Changed)
    ));
    drop(captured);
    fs::remove_file(fixture.root.join("note.txt")).unwrap();
    let deleted = fixture.capture(&["note.txt"]).unwrap();
    assert!(deleted.after[Path::new("note.txt")].is_none());
    drop(deleted);
    let base = b"hello world\n";
    let target = b"hello rust\n";
    let base_id = blob_hash(base);
    let target_id = blob_hash(target);
    fixture.pack(vec![
        (base_id, Encoding::Blob(base.to_vec())),
        (target_id, Encoding::Offset(0, delta())),
    ]);
    fixture.index(&[("nested/note.txt", target_id, 0o100644)]);
    let deleted = fixture.capture(&["nested/note.txt"]).unwrap();
    assert_eq!(deleted.before[Path::new("nested/note.txt")], target);
    assert!(deleted.after[Path::new("nested/note.txt")].is_none());
}
#[test]
fn protected_object_aliases_and_delta_bases_never_escape() {
    let fixture = Fixture::new();
    let base = b"hello world\n";
    let target = b"hello rust\n";
    let base_id = blob_hash(base);
    let target_id = blob_hash(target);
    fixture.pack(vec![
        (base_id, Encoding::Blob(base.to_vec())),
        (target_id, Encoding::Reference(base_id, delta())),
    ]);
    fixture.index(&[
        (".env", base_id, 0o100644),
        ("normal.txt", target_id, 0o100644),
    ]);
    fs::write(fixture.root.join("normal.txt"), b"working\n").unwrap();
    assert!(matches!(
        fixture.capture(&["normal.txt"]),
        Err(Error::Denied)
    ));
    fixture.index(&[
        (".env", target_id, 0o100644),
        ("normal.txt", target_id, 0o100644),
    ]);
    assert!(matches!(
        fixture.capture(&["normal.txt"]),
        Err(Error::Denied)
    ));
    assert!(matches!(fixture.capture(&[".env"]), Err(Error::Denied)));
    assert!(!Error::Denied.note().contains(".env") && !Error::Denied.note().contains("pack-"));
}
#[test]
fn cycles_and_depth_are_bounded_before_materialization() {
    let fixture = Fixture::new();
    fixture.pack(vec![
        ([1; 20], Encoding::Reference([2; 20], vec![0, 0])),
        ([2; 20], Encoding::Reference([1; 20], vec![0, 0])),
    ]);
    fixture.index(&[("note.txt", [1; 20], 0o100644)]);
    assert!(matches!(
        fixture.capture(&["note.txt"]),
        Err(Error::Invalid)
    ));
    let fixture = Fixture::new();
    let mut objects = Vec::new();
    for n in 1u8..=33 {
        objects.push(([n; 20], Encoding::Reference([n + 1; 20], vec![0, 0])));
    }
    objects.push(([34; 20], Encoding::Blob(Vec::new())));
    fixture.pack(objects);
    fixture.index(&[("note.txt", [1; 20], 0o100644)]);
    assert!(matches!(fixture.capture(&["note.txt"]), Err(Error::Limit)));
}
#[cfg(not(windows))]
#[test]
fn index_identity_and_digest_are_revalidated() {
    let fixture = Fixture::new();
    let id = fixture.loose(b"before");
    fixture.index(&[("note.txt", id, 0o100644)]);
    fs::write(fixture.root.join("note.txt"), b"after").unwrap();
    let captured = fixture.capture(&["note.txt"]).unwrap();
    let path = fixture.root.join(".git/index");
    let mut bytes = fs::read(&path).unwrap();
    bytes[12] ^= 1;
    rechecksum(&mut bytes);
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        captured.unchanged(&AtomicBool::new(false)),
        Err(Error::Changed)
    ));
    drop(captured);
    let captured = fixture.capture(&["note.txt"]).unwrap();
    fs::rename(&path, path.with_file_name("old-index")).unwrap();
    fs::write(&path, bytes).unwrap();
    assert!(matches!(
        captured.unchanged(&AtomicBool::new(false)),
        Err(Error::Changed)
    ));
}
#[cfg(windows)]
#[test]
fn captured_git_index_cannot_be_changed_while_its_handle_is_held() {
    let fixture = Fixture::new();
    let id = fixture.loose(b"before");
    fixture.index(&[("note.txt", id, 0o100644)]);
    fs::write(fixture.root.join("note.txt"), b"after").unwrap();
    let captured = fixture.capture(&["note.txt"]).unwrap();
    let path = fixture.root.join(".git/index");
    let mut bytes = fs::read(&path).unwrap();
    bytes[12] ^= 1;
    rechecksum(&mut bytes);

    // The Windows metadata handle deliberately denies write sharing, keeping
    // the captured index stable until the snapshot is released.
    assert!(fs::write(&path, bytes).is_err());
    assert!(captured.unchanged(&AtomicBool::new(false)).is_ok());
}
#[cfg(unix)]
#[test]
fn metadata_links_and_gitfile_pointers_are_refused() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let id = fixture.loose(b"before");
    fixture.index(&[("note.txt", id, 0o100644)]);
    let path = fixture.root.join(".git/index");
    let outside = fixture.root.join("outside-index");
    fs::rename(&path, &outside).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(fixture.capture(&["note.txt"]).is_err());
    fs::remove_file(&path).unwrap();
    fs::hard_link(&outside, &path).unwrap();
    assert!(fixture.capture(&["note.txt"]).is_err());
    let fixture = Fixture::new();
    fs::rename(fixture.root.join(".git"), fixture.root.join("metadata")).unwrap();
    fs::write(fixture.root.join(".git"), "gitdir: metadata\n").unwrap();
    assert!(fixture.capture(&["note.txt"]).is_err());
}

#[cfg(target_os = "macos")]
fn git(fixture: &Fixture, args: &[&str]) -> String {
    let executable = [
        "/Library/Developer/CommandLineTools/usr/bin/git",
        "/Applications/Xcode.app/Contents/Developer/usr/bin/git",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())
    .expect("supported local Git for inert fixture");
    let home = fixture.root.join("fixture-home");
    fs::create_dir_all(&home).unwrap();
    let output = std::process::Command::new(executable)
        .current_dir(&fixture.root)
        .env_clear()
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@local.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "inert Git fixture command failed");
    String::from_utf8(output.stdout).unwrap()
}
#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "Native qualified Git inspection over an inert real packed repository"]
async fn native_real_packed_repository_diff_is_current_index_scoped() {
    let fixture = Fixture::new();
    git(&fixture, &["init", "--quiet", "--initial-branch=fixture"]);
    let baseline = (0..500)
        .map(|n| format!("line {n}: stable fixture content\n"))
        .collect::<String>();
    fs::write(fixture.root.join("note.txt"), &baseline).unwrap();
    fs::write(fixture.root.join(".env"), "inert-private-fixture\n").unwrap();
    git(&fixture, &["add", "--", "note.txt", ".env"]);
    git(
        &fixture,
        &["commit", "--quiet", "--no-verify", "-m", "fixture zero"],
    );
    let old = git(&fixture, &["rev-parse", "HEAD:note.txt"])
        .trim()
        .to_owned();
    for n in 1..=5 {
        fs::write(
            fixture.root.join("note.txt"),
            format!("{baseline}revision {n}\n"),
        )
        .unwrap();
        git(&fixture, &["add", "--", "note.txt"]);
        git(
            &fixture,
            &["commit", "--quiet", "--no-verify", "-m", "fixture revision"],
        );
    }
    git(
        &fixture,
        &[
            "repack",
            "-a",
            "-d",
            "-f",
            "--window=50",
            "--depth=10",
            "--threads=1",
        ],
    );
    // Select an older, delta-compressed blob as the current index baseline.
    git(
        &fixture,
        &[
            "update-index",
            "--cacheinfo",
            &format!("100644,{old},note.txt"),
        ],
    );
    let captured = fixture.capture(&["note.txt"]).unwrap();
    assert_eq!(captured.before[Path::new("note.txt")], baseline.as_bytes());
    let metadata = GitMetadata::open(&fixture.grant, &fixture.root).unwrap();
    let decoded = index::parse(&captured.index.bytes, &|| Ok(())).unwrap();
    let mut objects = Objects::new(&metadata, &decoded.protected, Budget::new());
    let oid = decoded.entries[Path::new("note.txt")].oid;
    let packed = objects.pack_for(&oid).unwrap();
    assert!(
        matches!(
            packed.payload(&oid, &mut Budget::new()).unwrap(),
            pack::Payload::Delta { .. }
        ),
        "real fixture exercises packed deltas"
    );
    drop(captured);
    drop(objects);
    drop(metadata);
    // Poison source config/hooks only after building the inert fixture. Neither
    // metadata reaches the child; its own configuration/attributes are fixed.
    let marker = fixture.root.join("should-not-run");
    fs::write(fixture.root.join(".git/config"),format!("[core]\n repositoryformatversion = 0\n fsmonitor = touch {}\n[diff]\n external = touch {}\n",marker.display(),marker.display())).unwrap();
    fs::write(
        fixture.root.join(".git/hooks/pre-commit"),
        format!("#!/bin/sh\ntouch {}\n", marker.display()),
    )
    .unwrap();
    let result = inspect(
        &fixture.grant,
        &fixture.root,
        &["note.txt".into()],
        ReadOperation::Diff,
        || async { false },
    )
    .await
    .unwrap();
    assert!(result.success && result.text.contains("+revision 5"));
    assert!(!result.text.contains("inert-private-fixture") && !result.text.contains("pack-"));
    assert!(!marker.exists());
    fs::remove_file(fixture.root.join("note.txt")).unwrap();
    let result = inspect(
        &fixture.grant,
        &fixture.root,
        &["note.txt".into()],
        ReadOperation::Status,
        || async { false },
    )
    .await
    .unwrap();
    assert!(
        result.success && result.text.contains(" D note.txt"),
        "deletion stays unstaged against the imported index baseline"
    );
    assert!(!marker.exists());
}

#[test]
fn secret_index_and_current_content_are_refused() {
    let fixture = Fixture::new();
    let secret = b"ghp_inertFixtureToken";
    let secret_id = fixture.loose(secret);
    fs::write(
        fixture.root.join(".git/index"),
        index_bytes(&[("note.txt", secret_id, 0o100644)]),
    )
    .unwrap();
    fs::write(fixture.root.join("note.txt"), "ordinary").unwrap();
    assert!(matches!(fixture.capture(&["note.txt"]), Err(Error::Denied)));
    let ordinary = fixture.loose(b"ordinary");
    fs::write(
        fixture.root.join(".git/index"),
        index_bytes(&[("note.txt", ordinary, 0o100644)]),
    )
    .unwrap();
    fs::write(fixture.root.join("note.txt"), secret).unwrap();
    assert!(matches!(fixture.capture(&["note.txt"]), Err(Error::Denied)));
}

#[tokio::test]
async fn blocked_worker_has_absolute_deadline_and_cooperative_cancel() {
    let slots = Arc::new(super::super::worker_slots::Slots::new(1));
    let began = Instant::now();
    let result: Result<(), Error> = super::super::worker::run(
        &slots,
        &|| async { false },
        super::super::worker::Limits {
            filesystem: Duration::from_millis(20),
            total: Duration::from_secs(2),
            grace: Duration::from_millis(300),
        },
        |control| async move {
            while !control.cancelled.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(Error::Stopped)
        },
    )
    .await;
    assert_eq!(result, Err(Error::Limit));
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(slots.outstanding(), 0);
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "Qualified current-index class admission under pending worker debt; no Git starts"]
async fn native_git_index_pending_workers_hold_capacity_before_capture() {
    super::super::ensure_available().unwrap();
    super::super::worker::pending_capacity_fixture(slots().clone(), || async {
        matches!(
            inspect(
                &WorkFileGrant::default(),
                Path::new("/unused-fixture-path"),
                &["note.txt".into()],
                ReadOperation::Diff,
                || async { false }
            )
            .await,
            Err(Error::Runner(Failure::WorkerBusy))
        )
    })
    .await;
}
