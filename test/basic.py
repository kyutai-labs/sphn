import sphn

filename = "bria.mp3"
durations = sphn.durations([filename])
print(durations)

fr = sphn.FileReader(filename)
print(fr.sample_rate, fr.duration_sec, fr.channels)

data = fr.decode_all()
print(data.shape)
