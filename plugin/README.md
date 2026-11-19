# ast-index Agent Plugin

This directory is the shared payload for agent integrations.

## Supported Surfaces

- Claude Code: `.claude-plugin/plugin.json`, `commands/`, and `skills/`.
- Codex: `.codex-plugin/plugin.json` and `skills/`.
- Cursor: `.cursor-plugin/plugin.json`, `skills/`, `rules/`, and
  `commands-cursor/`.
- Grok Build: `.grok-plugin/plugin.json`, `skills/`, `commands-grok/`, and
  `hooks/hooks-grok.json`. The repo marketplace is
  `.grok-plugin/marketplace.json`.

The Claude commands are intentionally separate from the Cursor command because
they write different project configuration files.

For Claude Code, use `/initialize` as the default entrypoint. It auto-detects
single-stack, KMP, and polyglot repos, then configures `.claude/settings.json`
and `.claude/rules/ast-index.md`. The platform-specific `/initialize-*`
commands remain available as manual overrides.

## Local Testing

Codex can load the skill directly from `~/.codex/skills`:

```bash
mkdir -p ~/.codex/skills
ln -s /absolute/path/to/Claude-ast-index-search/plugin/skills/ast-index ~/.codex/skills/ast-index
```

The repo marketplace at `.agents/plugins/marketplace.json` is kept for Codex
builds that support plugin marketplaces.

Cursor can load the skill directly from `~/.cursor/skills`:

```bash
mkdir -p ~/.cursor/skills
ln -s /absolute/path/to/Claude-ast-index-search/plugin/skills/ast-index ~/.cursor/skills/ast-index
```

Cursor can load the plugin locally by symlinking this directory:

```bash
mkdir -p ~/.cursor/plugins/local
ln -s /absolute/path/to/Claude-ast-index-search/plugin ~/.cursor/plugins/local/ast-index
```

Then reload Cursor and verify the `ast-index` skill, rule, and
`initialize-ast-index` command appear. For Claude Code plugin testing, verify
that `/initialize` and the manual override `/initialize-*` commands appear.

Validate the Grok manifest from the repo root:

```bash
grok plugin validate plugin
```

The Grok command is `/ast-index-initialize`. It writes `.grok/rules/ast-index.md`
and does not use the Claude `/initialize` command files.
