# User docs: rules for agents and contributors

Everything in this folder (except this file, CLAUDE.md and README.md) is published at
https://getartcraft.com/docs/soundcraft. The file path is the URL:
`troubleshooting/soundcraft-does-not-open-on-windows.md` → `/docs/soundcraft/troubleshooting/soundcraft-does-not-open-on-windows`.
Renaming a file breaks links, so add the old path to `redirect_from` when you rename.

## Who reads this

Artists using SoundCraft, Discord mods answering them, and AI assistants looking things up for
either. Write for a musician or audio editor, not a developer.

## Page rules

- One question or problem per page. Lead with the answer or the fix in the first two lines.
- Plain words, short sentences, numbered steps for anything the reader does.
- Start the body at `##`. The page title comes from frontmatter.
- Link to other pages instead of repeating them. Use relative links to the `.md` file:
  `[Install SoundCraft](../getting-started/install.md)`.
- No marketing copy, no "simply", no claims you have not checked in the code or a release.
- Call the product "SoundCraft", as the app, its downloads and getartcraft.com do.
- Name UI labels exactly as the app shows them, in **bold**. Menu labels come from
  `crates/engine/catalog/menus.txt` and the command specs in `crates/engine/src/cmd/`; dialogs and windows
  are in `crates/ui-egui/src/`.
- Windows 11 is the only Windows version these docs name.
- Troubleshooting pages: `## Symptoms`, `## Fix`, `## Why this happens` (optional),
  `## Still stuck?` (link the issue tracker or Discord).

## Frontmatter

    ---
    title: SoundCraft does not open on Windows
    description: One sentence for search results, link previews and AI summaries.
    order: 10                 # optional, sorts within the folder (lower first)
    verified: 0.6.1           # optional, app version you last checked this page against
    redirect_from: [old/path] # optional, old URLs (relative to /docs/soundcraft)
    ---

Only `key: value` and `key: [a, b]` lines work. Quote values that start with a quote or contain ` #`.

Set `verified` to the latest release (the newest release tag) after checking the page against that tag with `git show <tag>:<path>`. Pages describe the latest release users download, not unreleased code on main.

## Folders

- `getting-started/` install and first run
- `guides/` how to do one thing
- `troubleshooting/` symptom → fix
- `faq/` short answers
- `reference/` lists and settings

A folder's `index.md` names the folder in the sidebar. Files or folders starting with `_` are
drafts and are not published.

## Check before you push

    node <path to your artcraft-services checkout>/frontend/apps/artcraft-website-next/scripts/docs/core.mjs docs/user

The checker lives in storytold/artcraft-services. A CI check will be added once it is on that repo's main branch.

## Support PRs

Pages written to answer a user question come in as PRs titled `docs(support): <page title>`.
Review them like code: check every claim against the source.
