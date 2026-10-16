# ADR-0003 — Configuration format: TOML

**Status:** Accepted
**Date:** 2026-10-16
**Depends on:** ADR-0001 (license)

> Supersedes the earlier KDL decision taken during M0 scaffolding. The KDL choice
> was a maintainer recommendation that the project owner revised the same day, before
> the config surface was public; it is rewritten here rather than kept as a
> superseded record because it was never ratified. (If ADRs are ever frozen, this
> becomes "supersedes ADR-0003-KDL".)

## Context

`COMPOSITOR.md` §13 and `ROADMAP.md` assume a human-editable text config with
hot-reload, good error messages, and tables for tags/rules/binds. The realistic
options were TOML, KDL, YAML, and RON.

Relevant prior art:

- **niri** uses **KDL** (via `knuffel`, the KDL **v1** dialect).
- **cosmic-comp** uses **RON** (a Rust-native format) with its own schema tooling.
- The initial M0 scaffold chose KDL (v2), but the maintainer prefers **TOML** for its
  ubiquity, tooling, and familiarity to users of most other software.

## Decision

crawm's configuration is **TOML**, parsed with `serde` and the `toml` crate
(`crawm-config`).

## Consequences

- **Ubiquitous and tooled:** every editor, linter (`taplo`), and formatter supports
  TOML out of the box; no bespoke syntax highlighting is required.
- **Familiar:** users arriving from almost any other application can read and write it
  immediately, unlike KDL or RON.
- `serde` deserialization into a `RawConfig` (all-optional) gives **defaults for
  free**, ignores unknown keys for forward compatibility, and reports
  present-but-wrongly-typed keys with a byte-precise error message
  (`toml`'s diagnostics point at the exact line and column).
- Hot reload stays trivially atomic: parse the whole file into a fresh `Config` and
  swap only if it validates.
- **Trade-off:** TOML is less pleasant for *repeated, deeply-nested* structures (long
  lists of window rules, per-tag bind tables). We mitigate this with arrays of tables
  (`[[window_rule]]`, `[[bind]]`) and by keeping such tables shallow; if a construct
  becomes genuinely awkward we will reconsider a nested format for that one table in a
  follow-up ADR rather than switching the whole config.
- Reserved keys use `snake_case` (e.g. `border_width`, `border_color`), matching
  `serde`'s field mapping and TOML convention; `border-color`-style keys are not used.

## Alternatives considered

- **KDL** — pleasant for repeated nodes and comments, and the niri precedent helps
  migration; rejected because it is far less familiar and less tooled than TOML for
  the audience crawm targets, and because kdl-rs now defaults to the KDL **v2** dialect
  (`#true`/`#false`) which further raises the unfamiliarity cost.
- **RON (cosmic's choice)** — excellent Rust ergonomics, but tying the user-facing
  format to Rust syntax is a poor fit for a config that scripts and other languages
  may generate.
- **YAML** — ubiquitous, but significant-whitespace and type-coercion pitfalls make it
  a poor fit for a config we must validate strictly.
