"""Pinned writable roots: their identity is kept between steps, never an open descriptor."""

import os

import pytest

from sakura.flow import pinned_roots


def _open_descriptors() -> int:
    return len(os.listdir("/dev/fd"))


def test_PinnedRoots_ManyPinnedFolders_KeepNoDescriptorOpenBetweenSteps(tmp_path):
    # A long engine process pins a folder for every run: descriptors kept for each ran out in the full suite.
    before = _open_descriptors()
    folders = []
    for number in range(50):
        folder = tmp_path / f"root-{number}"
        folder.mkdir()
        folders.append(pinned_roots.pin(folder))
        pinned_roots.make_folder(folder, "config")
        pinned_roots.verified(folder)
    try:
        assert _open_descriptors() == before
    finally:
        for folder in folders:
            pinned_roots.release(folder)


def test_PinnedRoots_AFolderReplacedByAnotherRealFolder_IsRefusedWhenMadeInto(tmp_path):
    folder = tmp_path / "root"
    folder.mkdir()
    pinned_roots.pin(folder)
    folder.rename(tmp_path / "moved")
    folder.mkdir()
    try:
        with pytest.raises(pinned_roots.RootReplaced, match="another folder"):
            pinned_roots.make_folder(folder, "config")
        assert not (folder / "config").exists()
    finally:
        pinned_roots.release(folder)
