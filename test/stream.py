import numpy as np
import sphn

filename = "bria.mp3"
data, sr = sphn.read(filename)
print(data.shape, sr)

data = sphn.resample(data, sr, 48000)
print(data.shape)

stream_writer = sphn.OpusStreamWriter(48000)
# This must be an allowed value among 120, 240, 480, 960, 1920, and 2880.
packet_size = 960
for lo in range(0, data.shape[-1], packet_size):
    up = lo + packet_size
    packet = data[0, lo:up]
    print("WRITER", packet.shape)
    if packet.shape[-1] != packet_size:
        break
    stream_writer.append_pcm(packet)

stream_reader = sphn.OpusStreamReader(48000)
all_pcms = []
with open("myfile.opus", "wb") as fobj:
    while True:
        opus = stream_writer.read_bytes()
        if len(opus) == 0:
            break
        fobj.write(opus)
        stream_reader.append_bytes(opus)
        pcm = stream_reader.read_pcm()
        if pcm.shape[0] > 0:
            print("READER", pcm.shape)
            all_pcms.append(pcm)

# This closes the input side of the stream, we can still read the pending pcm data.
stream_reader.close()
while True:
    pcm = stream_reader.read_pcm()
    if pcm is None:
        break
    if pcm.shape[0] > 0:
        print("READER-POST", pcm.shape)
        all_pcms.append(pcm)

all_pcms = np.concatenate(all_pcms)
print(all_pcms.shape)

data_roundtrip, sr_roundtrip = sphn.read_opus("myfile.opus")
print(data_roundtrip.shape, sr_roundtrip)
sphn.write_opus("myfile2.opus", all_pcms, 48000)

