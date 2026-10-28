"""Verify existing release artifacts without rebuilding or publishing them."""
import base64
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tomllib
import zipfile
from datetime import datetime, timezone

REPO = "zephium-browser/Zephium"
TAG = "v1.0.4"
SOURCE = "1a4c039b7de4c61bc7905bffbf8859d6b5361fab"
RUN = 40728542253
CI = 40728464188
FILES = {
    "release-macos": {"Zephium-macOS-arm64.dmg", "Zephium-macOS-arm64.app.tar.gz", "Zephium-macOS-arm64.app.tar.gz.sig"},
    "release-windows": {"Zephium-Windows-x64-setup.exe", "Zephium-Windows-x64-setup.exe.sig"},
}


def require(condition, message):
    if not condition:
        raise SystemExit(message)


def api(path):
    return json.loads(subprocess.check_output(["gh", "api", f"repos/{REPO}/{path}"]))


def source_file(path):
    return base64.b64decode(api(f"contents/{path}?ref={SOURCE}")["content"])


def check_tag():
    ref = api(f"git/ref/tags/{TAG}")["object"]
    require(ref["type"] == "tag", "Expected an annotated release tag")
    target = api(f"git/tags/{ref['sha']}")["object"]
    require(target["type"] == "commit" and target["sha"] == SOURCE, "Release tag moved")


def main():
    check_tag()
    run = api(f"actions/runs/{RUN}")
    require(run["head_sha"] == SOURCE and run["head_branch"] == TAG
            and run["event"] == "push" and run["path"] == ".github/workflows/release.yml",
            "Artifacts must come from the original tagged release workflow")
    jobs = api(f"actions/runs/{RUN}/jobs?filter=latest&per_page=100")
    require(jobs["total_count"] <= 100, "Incomplete job listing")
    for name in ("Gate", "Main CI for the release commit", "macOS (Apple Silicon)", "Windows (x64)"):
        matching = [job for job in jobs["jobs"] if job["name"] == name]
        require(len(matching) == 1 and matching[0]["conclusion"] == "success", f"Required original job failed: {name}")
    ci = api(f"actions/runs/{CI}")
    require(ci["head_sha"] == SOURCE and ci["head_branch"] == "main" and ci["event"] == "push"
            and ci["path"] == ".github/workflows/ci.yml" and ci["conclusion"] == "success", "Exact main CI did not pass")
    version = tomllib.loads(source_file("desktop/Cargo.toml").decode())["package"]["version"]
    require(TAG == f"v{version}", "Version does not match the tag")
    public_key = json.loads(source_file("desktop/tauri.conf.json"))["plugins"]["updater"]["pubkey"]
    dist = Path("dist")
    dist.mkdir()
    scratch = Path(os.environ["RUNNER_TEMP"]) / "release-recovery"
    scratch.mkdir()
    artifacts = api(f"actions/runs/{RUN}/artifacts?per_page=100")
    require(artifacts["total_count"] <= 100, "Incomplete artifact listing")
    evidence = []
    for name, expected in FILES.items():
        matching = [artifact for artifact in artifacts["artifacts"] if artifact["name"] == name]
        require(len(matching) == 1, f"Missing or ambiguous artifact: {name}")
        artifact = matching[0]
        require(not artifact["expired"] and artifact["workflow_run"]["head_sha"] == SOURCE
                and artifact["workflow_run"]["id"] == RUN, "Artifact source mismatch")
        archive = scratch / f"{name}.zip"
        with archive.open("wb") as output:
            subprocess.run(["gh", "api", f"repos/{REPO}/actions/artifacts/{artifact['id']}/zip"], stdout=output, check=True)
        digest = "sha256:" + hashlib.sha256(archive.read_bytes()).hexdigest()
        require(digest == artifact["digest"], "Downloaded artifact digest mismatch")
        with zipfile.ZipFile(archive) as zipped:
            entries = zipped.infolist()
            require(len(entries) == len(expected) and {entry.filename for entry in entries} == expected,
                    "Unexpected files in release archive")
            for entry in entries:
                require(not entry.is_dir() and not stat.S_ISLNK(entry.external_attr >> 16)
                        and entry.file_size < 1024 * 1024 * 1024, "Invalid archive entry")
                (dist / entry.filename).write_bytes(zipped.read(entry))
        evidence.append({"id": artifact["id"], "name": name, "sha256": digest})
    key = scratch / "updater.pub"
    key.write_bytes(base64.b64decode(public_key, validate=True))
    for filename in ("Zephium-macOS-arm64.app.tar.gz", "Zephium-Windows-x64-setup.exe"):
        signature = scratch / f"{filename}.minisig"
        signature.write_bytes(base64.b64decode((dist / f"{filename}.sig").read_text().strip(), validate=True))
        subprocess.run(["minisign", "-V", "-p", str(key), "-m", str(dist / filename), "-x", str(signature)], check=True)
        if filename.endswith(".exe"):
            require(signature.read_text().splitlines()[2] ==
                    f"trusted comment: zephium-updater-v1 version={version} target=windows-x86_64-nsis",
                    "Windows signature lacks the exact version and target binding")
    base = f"https://github.com/{REPO}/releases/download/{TAG}"
    platforms = {}
    for targets, filename in ((["darwin-aarch64", "darwin-aarch64-app"], "Zephium-macOS-arm64.app.tar.gz"),
                              (["windows-x86_64", "windows-x86_64-nsis"], "Zephium-Windows-x64-setup.exe")):
        for target in targets:
            platforms[target] = {"signature": (dist / f"{filename}.sig").read_text().strip(), "url": f"{base}/{filename}"}
    (dist / "latest.json").write_text(json.dumps({"version": version, "notes": f"https://github.com/{REPO}/releases/tag/{TAG}",
        "pub_date": datetime.now(timezone.utc).isoformat(), "platforms": platforms}, indent=2) + "\n")
    manifest = {"sourceCommit": SOURCE, "tag": TAG, "originalReleaseRun": RUN, "mainCiRun": CI,
        "recoveryRun": os.environ["GITHUB_RUN_ID"], "recoveryWorkflowCommit": os.environ["GITHUB_SHA"],
        "artifacts": evidence, "componentValidationWaived": True,
        "componentJobs": [{"name": job["name"], "status": job["status"], "conclusion": job["conclusion"]}
                          for job in jobs["jobs"] if job["name"].startswith("components /")],
        "files": {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(dist.iterdir())}}
    (dist / "release-recovery.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (dist / "SHA256SUMS").write_text("".join(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n" for path in sorted(dist.iterdir())))
    check_tag()
    print(f"Verified existing {TAG} artifacts from {SOURCE}; draft creation only.")


if __name__ == "__main__":
    main()
