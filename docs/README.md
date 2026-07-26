# Misy Documentation

## Table of Contents

- [Purpose](#purpose)
- [Navigation](#navigation)
- [Source of Truth](#source-of-truth)

## Purpose

This directory holds stable project knowledge for maintainers and future agents. Task plans, handoff notes, and temporary review findings do not belong here.

## Navigation

- [Architecture](architecture.md) — core boundaries, ownership, lifecycle, and data flow.
- [Provider Plugins](provider-plugins.md) — package layout, discovery, process protocol, and provider responsibilities.
- [TUI Client](tui-client.md) — client boundaries, interaction model, and keyboard behavior.
- [Development and Testing](development-and-testing.md) — workflow and verification commands.
- [Decisions and References](decisions-and-references.md) — fixed decisions, deferred scope, and use of Codex/Oh My Pi references.

The root [README](../README.md) is the user-facing project introduction.

## Source of Truth

Code and executable tests are authoritative for implemented behavior. These documents describe stable boundaries and must be updated when those boundaries change.
