# CLAUDE.md

@AGENTS.md

## Claude Code specifics

**The Rust skill**
- `.claude/skills/rust-best-practices/` is a project skill.
- Load it, plus the relevant chapters, before writing or reviewing any non-trivial Rust.

**The git policy is enforced**
- A PreToolUse hook (`.claude/hooks/block-git-writes.sh`) plus deny rules in `.claude/settings.json` block git and gh write commands.
- If a command is blocked, don't retry it in another form. Report what you would have run, and let the human do it.

**Your own configuration**
- Don't edit `.claude/settings.json`, `.claude/hooks/`, or the vendored skill unless the human asks.

**Commands that need approval**
- Host-networking, kernel, and cluster commands are set to ask for approval.
- Approval covers that one command. It doesn't lift the rules in AGENTS.md §1.

**Plan mode**
- Use plan mode before any change that affects phase scope or ADRs in `PLAN.md`.
