#!/usr/bin/env python3
"""Workspace container. Prints the commit HEAD names in the workspace (a ref lookup).
usage: head.py"""
import boxgit

print(boxgit.workspace_head())
