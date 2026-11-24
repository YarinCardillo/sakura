#!/usr/bin/env python3
"""Validate and preserve cumulative Sparkle release items across feed generation."""

import copy
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

SPARKLE = "http://www.andymatuschak.org/xml-namespaces/sparkle"
ET.register_namespace("sparkle", SPARKLE)


def version(item):
    text = item.findtext(f"{{{SPARKLE}}}version")
    enclosure = item.find("enclosure")
    return text or (enclosure.get(f"{{{SPARKLE}}}version") if enclosure is not None else None)


def read(path):
    tree = ET.parse(path)
    channel = tree.getroot().find("channel")
    if channel is None or not channel.findall("item"):
        sys.exit("The previous appcast must contain a release history")
    if any(not version(item) for item in channel.findall("item")):
        sys.exit("An appcast item has no build version")
    return tree, channel


def main():
    mode, previous, target = sys.argv[1:]
    _, old = read(previous)
    if mode == "check":
        if any(version(item) == target for item in old.findall("item")):
            sys.exit("This build version already exists in the previous appcast")
        return
    tree, channel = read(target)
    old_items = old.findall("item")
    old_versions = {version(item) for item in old_items}
    for item in list(channel.findall("item")):
        if version(item) in old_versions:
            channel.remove(item)
    for item in old_items:
        channel.append(copy.deepcopy(item))
    tree.write(Path(target), encoding="utf-8", xml_declaration=True)


if __name__ == "__main__":
    main()
