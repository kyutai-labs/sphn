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

sphn.write_wav("bria.wav", data[0], sr)
sphn.write_opus("bria.opus", data[0], sr, resample_to=48000)
