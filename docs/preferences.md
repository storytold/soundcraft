# Preferences

Open **Setup > Preferences...** or press **Ctrl+Alt+,** (Windows/Linux) or
**Cmd+Option+,** (macOS). The window links to Appearance, Playback Engine and Keyboard
Shortcuts, and configures additional desktop CLAP and VST3 plugin folders.
Preferences has its own shortcut regardless of selection. **Identify Sync Point**
retains `Cmd/Ctrl+,`.

Enter one absolute folder path per line, or use **Add folder…**. **Apply** updates
the preferences; **Revert** restores the applied list. Closing the window discards
unapplied edits. Applied folders are saved with the other user settings on normal
exit and registered on the next launch. Restart SoundCraft after changing them.
Standard locations and `CLAP_PATH` / `VST3_PATH` remain in use. Removing a custom
folder does not remove plugins also found through those other locations.

Folder selection, Apply and startup registration do not scan folders or load plugin
binaries. The configured folders are searched lazily with the standard locations
when plugins are first requested. Web builds do not
support local native plugin folders. Up to 32 folder entries of at most 4096 bytes
each are accepted; blank lines and duplicate paths are ignored.

Only the Preferences dialog can change plugin folders; `ui.set` rejects patches
containing `plugin_folders`, including through the control channel and MCP.
The existing `setup.preferences` engine
query remains headless; `window.preferences` explicitly opens/closes the window
with `{"value":true}` / `{"value":false}`.
