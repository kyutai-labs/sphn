import sphn

filename = "bria.mp3"
data, sr = sphn.read(filename)
print(data.shape, sr)

data = sphn.resample(data, sr, 24000)
print(data.shape)

stream_writer = sphn.OpusStreamWriter(24000)
# This must be an allowed value among 120, 240, 480, 960, 1920, and 2880.
packet_size = 960
for lo in range(0, data.shape[-1], packet_size):
    up = lo + packet_size
    packet = data[:, lo:up]
    print(packet.shape)
    stream_writer.append_pcm(packet[0])
