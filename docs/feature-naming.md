# Feature naming migration

Status: draft, 2026-11-08. This is an engineering naming policy and rollout plan, not trademark clearance.

## Scheme

Use short, descriptive names for our own features. Built-in processing menus use the names in our processor registry, rather than names of another vendor's processors. Keep comparison catalogs and compatibility keys separate from display text. Labels must not determine dispatch, storage or processor selection.

| Previous visible name | Display name | Compatibility retained |
| --- | --- | --- |
| AudioSuite | Offline Processing | `audiosuite.process`, catalog paths, UI state |
| Beat Detective | Beat Analysis | `event.beat_detective`, `window.beat_detective` |
| Elastic Audio / Elastic Properties | Real-Time Stretch / Real-Time Stretch Properties | `track.elastic`, `clip.elastic_properties`, saved track values |
| x-form | Rendered (High Quality) | serialized `x-form` algorithm value |
| QuickPunch / TrackPunch | Punch Recording / Track Punch Recording | option ids and stored fields |
| Ableton Link | Tempo Sync | existing display flag only, no claim of Link interoperability; disabled placeholder |
| Pro Tools Search / Pro Tools Projects | Search / Shared Projects | historical catalog paths |
| Melodyne | No ARA plug-ins | disabled ARA placeholder, no added implementation |
| HEAT / Activate HEAT | Analog Saturation / Activate Analog Saturation | disabled catalog entries |
| Sibelius export/send | MusicXML | command ids; actual export format unchanged |

The stretch algorithm labels describe the existing behavior. This change does not add a distinct third-party algorithm, synchronization protocol, ARA integration or hosted service.

## Disabled placeholders

Four features we do not have keep their five catalog leaves: Tempo Sync (the incumbent's Ableton Link item), No ARA plug-ins (Track and Clip › ARA Plugins › Melodyne) and Analog Saturation (View › Mix Window Views › HEAT and Options › Activate HEAT). They keep the incumbent's place in the menu, and therefore the parity floor, but they are never interactive: the menu draws them disabled with a "Not available yet." hover, `item_state` refuses them whatever their command would allow, and no label claims the protocol, plug-in format or processor behind them. "No ARA plug-ins" says what the item is: a place where plug-ins of that kind would be listed once a host exists. Preferences already has the same rule for settings without a backend.

Adding a placeholder means adding its catalog path to `PLACEHOLDERS` in `crates/engine/src/catalog.rs`; an engine test checks that each one is still a catalog entry with a descriptive label, and a UI test opens each one and checks that it is still in the menu and stays disabled. A placeholder becomes a feature by wiring a command and removing its path from that list.

## Offline processor names

| Previous catalog item | Our processor name / id |
| --- | --- |
| EQ3 1-Band / EQ3 7-Band | EQ 1-Band / `eq_1band`; EQ 7-Band / `eq_7band` |
| BF-76 / Dyn3 Compressor/Limiter | Compressor/Limiter / `compressor` |
| Dyn3 De-Esser | De-Esser / `de_esser` |
| Dyn3 Expander/Gate | Expander/Gate / `expander_gate` |
| Maxim | Maximizer / `maximizer` |
| Pitch II / Pitch Shift Legacy | Pitch Shift / `pitch_shift` |
| Time Shift / Time Compression Expansion | Time Stretch / `time_stretch` |
| Vari-Fi | Speed Ramp / `speed_ramp` |
| D-Verb | Room Reverb / `room_reverb` |
| Mod Delay III | Mod Delay / `mod_delay` |
| Sci-Fi | Ring Modulator / `ring_mod` |
| Eleven Lite / SansAmp PSA-1 | Saturator / `saturator` |
| Lo-Fi | Bit Crusher / `lofi` |
| Recti-Fi | Rectifier / `rectifier` |

Other offline items already have descriptive names. Multiple legacy entries mapping to the same processor appear once per menu category. Every historical catalog path remains invokable. The catalog remains an observed interoperability/parity record; it is not used verbatim for rendered labels. Changing `lofi`'s display name does not change its id, parameters or audio implementation.

## Product-name decision

Deferred by the owner. The SoundCraft product name remains unchanged; this work only changes feature labels.

The SoundCraft app name has a separate clearance concern involving the established audio brand Soundcraft. A candidate for review is **ArtCraft Audio**, a descriptive extension of the existing suite identity. It is only a proposal; no availability or legal clearance is claimed. Avoid adopting another audio product name simply because it ends in “Craft.”

A product rename needs an owner decision and a separate migration PR covering app/window titles, website and release copy, package/binary names, bundle/application ids, installers, desktop files, update feeds, settings locations, file associations, shortcuts, URLs and external integrations. Preserve old command/package aliases and discover existing user data rather than resetting it. Verify the existing ArtCraft mark license applies to the new context before changing any brand assets. This feature-label draft makes none of those product-identity changes.

## Validation and release gates

- Regression tests cover old catalog-to-command mappings, own processor names, deduplication and continued opening of the Beat Analysis dialog through its legacy menu path. Guards scan the rendered menu tree, UI/engine command labels and processor display names for retired labels; compatibility paths and ids are excluded, and unknown display text still falls back unchanged.
- Menu tree identity uses the stable path, not the rendered name. Preferences keep their existing section identity while drawing and searching the descriptive name.
- This main branch currently ships an English-only interface. Carry the mapping into pending localization PRs as they land; do not reintroduce vendor marks in translated labels.
- Run `cargo xtask ci` on the final head. Exercise offline processing from the menus, old automation paths, opening saved sessions containing `lofi` and `x-form`, Undo/Redo, and preference search.
- Recapture the UI with the supported `ui_shot` example. `docs/images/edit.png` has been visually inspected and still shows the old processing-menu title. Inspect and refresh all affected README images and attribution records from the running app; unchanged screenshots are not claimed to be refreshed by this draft.
- Review current help/marketing copy and localization work after integration. Historical parity documents, API examples and factual interoperability references may retain old names with their context. Preserve license/attribution records.
