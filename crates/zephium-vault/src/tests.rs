use std::fs;

use zephium_core::vault::{VaultEntryKind, VaultError};

use crate::{natural_cmp, NameIndex, Vault, TRASH};

fn vault() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("DBKF/Year 2/Sem 2/ITF/CA1")).unwrap();
    fs::write(
        dir.path().join("DBKF/Year 2/Sem 2/ITF/CA1/Brief.pdf"),
        b"%PDF-1.7",
    )
    .unwrap();
    fs::create_dir(dir.path().join(".obsidian")).unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    (dir, vault)
}

#[test]
fn lists_folders_first_naturally_without_hidden() {
    let (dir, vault) = vault();
    for name in ["Topic 10", "Topic 2"] {
        fs::create_dir(dir.path().join(name)).unwrap();
    }
    fs::write(dir.path().join("a.md"), "x").unwrap();
    let names: Vec<_> = vault
        .list("")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["DBKF", "Topic 2", "Topic 10", "a.md"]);
    let pdf = &vault.list("DBKF/Year 2/Sem 2/ITF/CA1").unwrap()[0];
    assert_eq!(pdf.ext, "pdf");
    assert_eq!(pdf.size, "8");
    assert_eq!(pdf.kind, VaultEntryKind::File);
}

#[test]
fn refuses_traversal_and_symlinks() {
    let (dir, vault) = vault();
    assert_eq!(vault.list("..").unwrap_err(), VaultError::Invalid);
    assert_eq!(vault.list("DBKF/../..").unwrap_err(), VaultError::Invalid);
    assert_eq!(
        vault.read("C:/Windows", 10).unwrap_err(),
        VaultError::Invalid
    );
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        assert_eq!(
            vault.read("link/secret", 10).unwrap_err(),
            VaultError::Invalid
        );
        assert!(vault.list("").unwrap().iter().all(|e| e.name != "link"));
    }
}

#[test]
fn creates_with_free_names() {
    let (_dir, vault) = vault();
    assert_eq!(
        vault.create_file("", "Note.md", b"# a").unwrap().path,
        "Note.md"
    );
    assert_eq!(
        vault.create_file("", "Note.md", b"# b").unwrap().path,
        "Note 1.md"
    );
    assert_eq!(vault.create_folder("DBKF", "New").unwrap().path, "DBKF/New");
    assert_eq!(
        vault.create_folder("DBKF", "New").unwrap().path,
        "DBKF/New 1"
    );
}

#[test]
fn renames_moves_and_trashes() {
    let (dir, vault) = vault();
    let pdf = "DBKF/Year 2/Sem 2/ITF/CA1/Brief.pdf";
    let renamed = vault.rename(pdf, "CA1 Brief.pdf").unwrap();
    assert_eq!(renamed.path, "DBKF/Year 2/Sem 2/ITF/CA1/CA1 Brief.pdf");
    assert_eq!(
        vault.rename(&renamed.path, "..").unwrap_err(),
        VaultError::Invalid
    );
    let moved = vault.move_to(&renamed.path, "DBKF").unwrap();
    assert_eq!(moved.path, "DBKF/CA1 Brief.pdf");
    assert_eq!(
        vault.move_to("DBKF", "DBKF/Year 2").unwrap_err(),
        VaultError::Invalid
    );
    vault.trash("DBKF/CA1 Brief.pdf").unwrap();
    assert!(dir.path().join(TRASH).join("CA1 Brief.pdf").exists());
    assert_eq!(
        vault.stat("DBKF/CA1 Brief.pdf").unwrap_err(),
        VaultError::NotFound
    );
}

#[test]
fn imports_downloads() {
    let (_dir, vault) = vault();
    let downloads = tempfile::tempdir().unwrap();
    let file = downloads.path().join("Lecture 3.pdf");
    fs::write(&file, b"pdf").unwrap();
    let entry = vault.import(&file, "DBKF").unwrap();
    assert_eq!(entry.path, "DBKF/Lecture 3.pdf");
    assert!(file.exists(), "files from outside are copied");
    let again = vault.import(&file, "DBKF").unwrap();
    assert_eq!(again.path, "DBKF/Lecture 3 1.pdf");
}

#[test]
fn writes_only_unchanged_files() {
    let (_dir, vault) = vault();
    let path = "DBKF/Year 2/Sem 2/ITF/CA1/Brief.pdf";
    let (bytes, modified) = vault.read(path, 1024).unwrap();
    assert_eq!(bytes, b"%PDF-1.7");
    assert_eq!(vault.read(path, 4).unwrap_err(), VaultError::TooLarge);
    let next = vault.write(path, b"%PDF-annotated", &modified).unwrap();
    assert_eq!(vault.read(path, 1024).unwrap().0, b"%PDF-annotated");
    assert_eq!(
        vault.write(path, b"x", "1").unwrap_err(),
        VaultError::Changed
    );
    assert!(!next.is_empty());
}

#[test]
fn searches_names() {
    let (_dir, vault) = vault();
    let mut index = NameIndex::default();
    let hits = index.search(&vault, "itf ca1", 10, false);
    assert_eq!(hits[0].path, "DBKF/Year 2/Sem 2/ITF/CA1");
    let folders = index.search(&vault, "brief", 10, true);
    assert!(folders.is_empty());
    let files = index.search(&vault, "brief", 10, false);
    assert_eq!(files[0].name, "Brief.pdf");
}

#[test]
fn natural_order() {
    assert!(natural_cmp("Topic 2", "Topic 10").is_lt());
    assert!(natural_cmp("a", "B").is_lt());
    assert!(natural_cmp("Sem 1", "Sem 1").is_eq());
}
