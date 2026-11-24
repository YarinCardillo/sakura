"""Where a contract flow may live (review 12): the host allowlist and the hard denials."""

import pytest

from sakura.flow import flow_paths

ALLOWED = ["tests/**", "test/**", "e2e/**", ".GitHub/**", ".github/**"]


@pytest.mark.parametrize("name", [".github/workflows/x.spec.yml", ".GitHub/workflows/x.yml", ".GITHUB/workflows/x.yml",
                                  "e2e/.GITATTRIBUTES", "e2e/Package.json", "tests/CONFTEST.py", "e2e/.Git/config",
                                  "tests/MAKEFILE"])
def test_Refusal_TheHardDenials_IgnoreCase(name):
    assert "configuration" in flow_paths.refusal(name, ALLOWED)


@pytest.mark.parametrize("name", ["Tests/flow.py", "E2E/flow.py"])
def test_Refusal_TheAllowlistStaysExact(name):
    assert "outside the contract flow folders" in flow_paths.refusal(name, ["tests/**", "e2e/**"])


def test_Refusal_AFlowInTheAllowedFolders_IsAccepted():
    assert flow_paths.refusal("e2e/help_flow.py", ["e2e/**"]) == ""


def test_Collisions_AFlowThatCollidesIgnoringCaseWithATrackedPathOrAnotherFlow_IsNamed():
    found = flow_paths.collisions(["e2e/README.md", "e2e/a.py", "e2e/A.py", "e2e/new.py"], ["e2e/readme.md", "app.py"])
    assert "e2e/README.md" in " ".join(found) and "e2e/A.py" in " ".join(found) and "e2e/new.py" not in " ".join(found)
