"""Prompts, docs and comments name the person with neutral words: no he, him, his, himself, she or her."""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FOLDERS = ("lib", "acceptance", "bin", "docs")
TOP_FILES = ("README.md", "BRIEF.md", "REVIEW.md")
SUFFIXES = {".md", ".py", ".toml", ".in", ".template", ""}
GENDERED = re.compile(r"\b(he|he's|him|his|himself|she|her|hers|herself)\b", re.IGNORECASE)


def _files():
    found = [ROOT / name for name in TOP_FILES if (ROOT / name).is_file()]
    for folder in FOLDERS:
        found += [path for path in (ROOT / folder).rglob("*")
                  if path.is_file() and path.suffix in SUFFIXES and "__pycache__" not in path.parts]
    return sorted(found)


def test_Wording_NoPromptDocOrCommentUsesAGenderedPronoun():
    hits = [f"{path.relative_to(ROOT)}:{number}: {match[0]}" for path in _files()
            for number, line in enumerate(path.read_text(errors="replace").splitlines(), start=1)
            for match in [GENDERED.search(line)] if match]
    assert hits == []
