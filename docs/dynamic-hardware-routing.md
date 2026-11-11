# Mixer hardware inputs and outputs

The Mix window's input selector lists the inputs of the selected, open recording device. Opening this menu starts the input stream if it is not already open; the stream is reused for recording and input monitoring. If the input cannot open, the status bar explains the error and the menu does not offer unavailable hardware inputs. Apply an audio-device change or use Reconnect audio device to refresh the stream.

The output selector lists the open playback device's physical channels as **Hardware Out** entries, separately from the **Main** mix and buses. A mono audio track can use any single physical output. Instruments render at least stereo, so their choices use that effective width. Master faders continue to process Main; non-audio tracks do not offer physical destinations. A stereo track offers consecutive pairs that fit inside the device; for example, a 48-channel device offers the last pair 47–48, not a pair that wraps from 48 back to 1. Wider track formats likewise require a complete consecutive span.

Hardware output paths preserve channel order and bypass the main mix's master processing. Signals routed to Main and a hardware path that addresses the same device channels are added together. The main speaker layout and its existing stereo/surround folding remain unchanged. Physical paths beyond the device width remain silent rather than being folded into unrelated outputs.

The menu reflects the device stream SoundCraft actually opened, which can be narrower than the maximum channel count advertised by the hardware. Physical routing supports up to 1024 channels; the separate speaker-layout limit is unchanged. Hardware channel lists scroll, so the last channels remain reachable on smaller screens.

Explicit hardware selections are distinct from buses with the same name. These paths use canonical channel names (`In N`, `Out N`). Configuring arbitrary named hardware paths through I/O Setup and changing the hardware configuration while recording are outside this change.
