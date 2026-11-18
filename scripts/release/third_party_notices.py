#!/usr/bin/env python3
"""Write the third-party notices shipped inside Zephium's installers.

Covers every non-workspace crate linked into the desktop app for the release
targets (including the vendored forks) and every production npm package in the
frame bundle, each with the license and notice texts it ships.
"""

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TARGETS = ("aarch64-apple-darwin", "x86_64-pc-windows-msvc")
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|copyright)([-_.].*)?$", re.IGNORECASE)
RULE = "=" * 78

# Several crates publish their license only at the repository root. Their
# SPDX terms are reproduced once here so every package's terms ship.
MIT = """Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE."""

BSD_2 = """Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE."""

BSD_3 = BSD_2.replace(
    "   and/or other materials provided with the distribution.\n",
    "   and/or other materials provided with the distribution.\n"
    "3. Neither the name of the copyright holder nor the names of its\n"
    "   contributors may be used to endorse or promote products derived from\n"
    "   this software without specific prior written permission.\n",
)

ZLIB = """This software is provided 'as-is', without any express or implied warranty.
In no event will the authors be held liable for any damages arising from the
use of this software.

Permission is granted to anyone to use this software for any purpose, including
commercial applications, and to alter it and redistribute it freely, subject to
the following restrictions:

1. The origin of this software must not be misrepresented; you must not claim
   that you wrote the original software. If you use this software in a product,
   an acknowledgment in the product documentation would be appreciated but is
   not required.
2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.
3. This notice may not be removed or altered from any source distribution."""


def license_texts(directory: Path) -> list[tuple[str, str]]:
    if not directory.is_dir():
        return []
    texts = []
    for path in sorted(directory.iterdir()):
        if path.is_file() and LICENSE_FILE.match(path.name):
            texts.append((path.name, path.read_text(encoding="utf-8", errors="replace").strip()))
    return texts


def rust_packages() -> dict[tuple[str, str], dict]:
    packages: dict[tuple[str, str], dict] = {}
    for target in TARGETS:
        metadata = json.loads(
            subprocess.run(
                ["cargo", "metadata", "--format-version", "1", "--locked",
                 "--filter-platform", target],
                cwd=ROOT, check=True, capture_output=True, text=True, encoding="utf-8",
            ).stdout
        )
        by_id = {package["id"]: package for package in metadata["packages"]}
        nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
        members = set(metadata["workspace_members"])
        root = next(id for id in members if by_id[id]["name"] == "zephium-desktop")
        # Only normal dependencies end up in the shipped binary.
        stack, seen = [root], {root}
        while stack:
            for dep in nodes[stack.pop()]["deps"]:
                if any(kind["kind"] is None for kind in dep["dep_kinds"]) and dep["pkg"] not in seen:
                    seen.add(dep["pkg"])
                    stack.append(dep["pkg"])
        for id in seen - members:
            package = by_id[id]
            packages[(package["name"], package["version"])] = package
    return packages


def npm_packages() -> list[dict]:
    listing = json.loads(
        subprocess.run(
            [shutil.which("pnpm") or "pnpm", "licenses", "list", "--prod", "--json"],
            cwd=ROOT / "frame", check=True, capture_output=True, text=True, encoding="utf-8",
        ).stdout
    )
    packages = []
    for license, entries in listing.items():
        for entry in entries:
            for version, path in zip(entry["versions"], entry["paths"]):
                packages.append({"name": entry["name"], "version": version,
                                 "license": license, "path": Path(path),
                                 "homepage": entry.get("homepage") or ""})
    return sorted(packages, key=lambda package: (package["name"], package["version"]))


def section(name: str, version: str, license: str, source: str, texts) -> str:
    lines = [RULE, f"{name} {version}", f"License: {license or 'see below'}"]
    if source:
        lines.append(f"Source: {source}")
    for file_name, text in texts:
        lines += ["", f"--- {file_name} ---", "", text]
    if not texts:
        lines += ["", "This package ships no separate license file; the full text of each",
                  "license named above is reproduced at the end of this document."]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    chromium = ROOT / "vendor/chromium-extension-api"
    sections = [section(
        "Chromium extension API schemas", "(pinned snapshot; see UPSTREAM.md)",
        "BSD-3-Clause", "https://chromium.googlesource.com/chromium/src/",
        [("LICENSE", (chromium / "LICENSE").read_text(encoding="utf-8").strip()),
         ("UPSTREAM.md", (chromium / "UPSTREAM.md").read_text(encoding="utf-8").strip())],
    )]
    for (name, version), package in sorted(rust_packages().items()):
        directory = Path(package["manifest_path"]).parent
        sections.append(section(name, version, package.get("license") or "",
                                package.get("repository") or "", license_texts(directory)))
    for package in npm_packages():
        sections.append(section(package["name"], package["version"], package["license"],
                                package["homepage"], license_texts(package["path"])))

    header = (
        "Zephium third-party notices\n\n"
        "Zephium is licensed under the Mozilla Public License 2.0. It includes the\n"
        "following third-party software, each under its own license. The bundled\n"
        "EasyList, EasyPrivacy and uBlock Origin filter lists are covered by\n"
        "licenses/blocker/.\n"
    )
    apache = (ROOT / "vendor/tauri/LICENSE_APACHE-2.0").read_text(encoding="utf-8").strip()
    mpl = (ROOT / "LICENSE").read_text(encoding="utf-8").strip()
    appendix = [RULE, "License texts for packages that ship none of their own"]
    for name, text in (("MIT", MIT), ("Apache-2.0", apache), ("BSD-2-Clause", BSD_2),
                       ("BSD-3-Clause", BSD_3), ("Zlib", ZLIB), ("MPL-2.0", mpl)):
        appendix += ["", f"--- {name} ---", "", text]
    sections.append("\n".join(appendix))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(header + "\n" + "\n\n".join(sections) + "\n", encoding="utf-8")
    print(f"{len(sections)} packages -> {args.output}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
