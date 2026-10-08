"""List tests that failed and then passed on a retry, from a nextest JUnit report."""

import os
import sys
import xml.etree.ElementTree as ET

try:
    root = ET.parse(sys.argv[1]).getroot()
except FileNotFoundError:
    sys.exit(0)

flaky = sorted(
    f"{case.get('classname')} {case.get('name')}"
    for case in root.iter("testcase")
    if case.find("flakyFailure") is not None or case.find("flakyError") is not None
)
for name in flaky:
    print(f"::warning title=Flaky test::{name} passed only on a retry")
summary = os.environ.get("GITHUB_STEP_SUMMARY")
if flaky and summary:
    with open(summary, "a", encoding="utf-8") as out:
        out.write("### Tests that passed only on a retry\n\n")
        out.writelines(f"- `{name}`\n" for name in flaky)
