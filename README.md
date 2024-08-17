# sphn

Python bindings for the [symphonia
crate](https://github.com/pdeljanov/Symphonia), easily load various audio file
formats into numpy arrays.

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
sphn.write_wav("bria.wav", data[0], sr)
```
