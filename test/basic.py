import numpy as np
import sphn

filename = "bria.mp3"
durations = sphn.durations([filename])
print(durations)

fr = sphn.FileReader(filename)
print(fr.sample_rate, fr.duration_sec, fr.channels)

data = fr.decode_all()
print(data.shape)

data, sr = sphn.read(filename)
print(data.shape, sr)

sphn.write_wav("bria_mono.wav", data[0], sr)
sphn.write_wav("bria_stereo.wav", np.concatenate([data, data]), sr)
sphn.write_opus("bria.opus", data, sr)

data_roundtrip, sr_roundtrip = sphn.read_opus("bria.opus")
assert sr_roundtrip == 48000, "sample rate from opus file is not 48khz"
data_resampled = sphn.resample(data, sr, sr_roundtrip)
