Tiny fixtures for `tests/it/decode_read_list.rs`: one per container or codec the READ list names
that the crate cannot write itself. Made with ffmpeg 8.0 from a 0.1 s mono 440 Hz sine
(`F="-y -f lavfi -i sine=frequency=440:duration=0.1 -ac 1"`):

    ffmpeg $F -ar 8000  -c:a libmp3lame -b:a 16k tone.mp3
    ffmpeg $F -ar 24000 -c:a mp2 -b:a 32k tone.mp2
    ffmpeg $F -ar 8000  -c:a libvorbis -q:a -1 tone.ogg
    ffmpeg $F -ar 8000  -c:a aac -b:a 16k tone.m4a
    ffmpeg $F -ar 8000  -c:a aac -b:a 16k -f adts tone.aac
    ffmpeg $F -ar 8000  -c:a pcm_s16le tone.caf
    ffmpeg $F -ar 8000  -c:a pcm_s16le tone.mkv
    ffmpeg $F -ar 8000  -c:a libvorbis -q:a -1 tone.webm
    ffmpeg $F -ar 8000  -c:a pcm_s16le -f aiff tone.aifc    # AIFF-C "sowt"

`silence.mp1` has no ffmpeg encoder. It is four hand-built MPEG-1 Layer I frames (mono, 32 kHz,
32 kbit/s, 48 bytes each: header `FF FF 18 C0`, all subband allocations zero), which decode to
silence.
