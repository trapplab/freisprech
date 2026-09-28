# Test audio

Raw PCM for `--file`: 16 kHz, mono, s16le. Listen: `ffplay -f s16le -ar 16000 -ch_layout mono <file>`

| File | Source | License | Expected text |
|---|---|---|---|
| `jfk.pcm` | [whisper.cpp `samples/jfk.wav`](https://github.com/ggml-org/whisper.cpp/blob/master/samples/jfk.wav), speech by J. F. Kennedy, 1961 | public domain (US government work) | And so my fellow Americans, ask not what your country can do for you, ask what you can do for your country. |

Convert your own files: `ffmpeg -i in.mp3 -ar 16000 -ac 1 -f s16le out.pcm`, or record one
from the microphone with `--record out.pcm`.
