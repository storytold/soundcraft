# Editing SoundCraft Help

Help ships with the app and reads two bundled JSON files.

- `commands.json` defines topic IDs in `categories`, pages keyed by registered command ID in `commands`, and offline processor pages in `audiosuite`. A command page needs a valid `category` and a plain-language `text`. Add `limits` when the app currently implements only part of the behavior. A processor page uses its registered `process` ID, a `title`, and `text`.
- `guides.json` defines guides with a unique `id`, a `title`, an existing `category`, and a `summary`. Put instructions in `steps`, or use `keys` for shortcut and explanation pairs. `related` lists command IDs that have their own pages.

In guide steps and key tables, `{menu:session.save}` renders a command's menu location, `{key:session.save}` renders its registered shortcut, and `{keys:Cmd+S}` renders an explicit chord. The renderer uses the platform's spelling for shortcuts. Use command and processor IDs from the registries so the catalog and links stay valid when visible labels change.

Run `cargo test -p soundcraft-ui-egui --lib help::tests` to check catalog coverage and guide references.
