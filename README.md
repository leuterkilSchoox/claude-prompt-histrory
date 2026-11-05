# Prompt History

Desktop viewer (Tauri v2) for Claude Code conversations stored in `~/.claude/projects/*/*.jsonl`.

- Sidebar grouped by **Project → Branch**, or flat by **Branch**
- Full-text search over prompts and Claude replies
- Chat bubbles, collapsible tool calls (toggle with "Tool calls")
- Export a conversation to `~/Downloads/<title>.md`
- The list refreshes when the window regains focus

## Install

Download the file for your OS from [Releases](https://github.com/leuterkilSchoox/claude-prompt-histrory/releases/latest):

| OS | File | Notes |
|----|------|-------|
| Ubuntu/Debian | `.deb` | `sudo dpkg -i Prompt*.deb` |
| Other Linux | `.AppImage` | `chmod +x` it and run |
| macOS | `.dmg` | Unsigned: after dragging to Applications run `xattr -cr "/Applications/Prompt History.app"` |
| Windows | `.msi` / `-setup.exe` | Unsigned: SmartScreen → "More info" → "Run anyway" |

## Release a new version

Bump `version` in `src-tauri/tauri.conf.json`, then `git tag vX.Y.Z && git push --tags`. GitHub Actions builds all installers and publishes the release.

## Develop

```bash
npm install
npm run dev      # dev window
npm run build    # .deb + AppImage in src-tauri/target/release/bundle/
cd src-tauri && cargo test
```

If the window is blank on Linux, start it with `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
