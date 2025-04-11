# sphn

Python bindings for the [symphonia](https://github.com/pdeljanov/Symphonia) and
[opus](https://github.com/SpaceManiac/opus-rs) crates.
- Easily load various audio file formats into numpy arrays.
- Read/write ogg/opus audio files with streaming support.

## Installation

The python wheels are available on [pypi](https://pypi.org/project/sphn/).

```bash
pip install sphn
```

If you're seeing an issue with CMake saying "Compatibility with CMake < 3.5 has been removed from CMake.",
this is because building Opus is not compatible with CMake 4.x. Install a 3.x version (e.g. 3.31)
and then set the `CMAKE` environment variable to use it, e.g.:

```bash
pip install 'cmake<4' # Or use your system's package manager
CMAKE=/path/to/.venv/bin/cmake pip install sphn
```

## Usage

Download some sample audio file.
```bash
wget https://github.com/metavoiceio/metavoice-src/raw/main/assets/bria.mp3
```

```python
import sphn

# Read an audio file
data, sample_rate = sphn.read("bria.mp3")
print(data.shape, sample_rate)

# Save as wav
sphn.write_wav("bria.wav", data[0], sample_rate)
```
