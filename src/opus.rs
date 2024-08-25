use anyhow::Result;

// This must be an allowed value among 120, 240, 480, 960, 1920, and 2880.
// Using a different value would result in a BadArg "invalid argument" error when calling encode.
// https://opus-codec.org/docs/opus_api-1.2/group__opus__encoder.html#ga4ae9905859cd241ef4bb5c59cd5e5309
const OPUS_ENCODER_FRAME_SIZE: usize = 960;
const OPUS_SAMPLE_RATE: u32 = 48000;
const OPUS_ALLOWED_FRAME_SIZES: [usize; 6] = [120, 240, 480, 960, 1920, 2880];

/// See https://www.opus-codec.org/docs/opusfile_api-0.4/structOpusHead.html
#[allow(unused)]
#[derive(Debug)]
struct OpusHeader {
    version: u8,
    channel_count: u8,
    pre_skip: u16,

    /// The sampling rate of the original input.
    ///
    /// All Opus audio is coded at 48 kHz, and should also be decoded at 48 kHz for playback (unless
    /// the target hardware does not support this sampling rate). However, this field may be used to
    /// resample the audio back to the original sampling rate, for example, when saving the output
    /// to a file.
    input_sample_rate: u32,
    output_gain: i16,
    mapping_family: u8,
}

fn parse_opus_header(packet: &[u8]) -> Result<OpusHeader> {
    if packet.len() < 8 || &packet[0..8] != b"OpusHead" {
        anyhow::bail!("not a OpusHead packet")
    }
    let header = OpusHeader {
        version: packet[8],
        channel_count: packet[9],
        pre_skip: u16::from_le_bytes([packet[10], packet[11]]),
        input_sample_rate: u32::from_le_bytes([packet[12], packet[13], packet[14], packet[15]]),
        output_gain: i16::from_le_bytes([packet[16], packet[17]]),
        mapping_family: packet[18],
    };
    Ok(header)
}

/// Read an ogg stream using the opus codec.
pub fn read_ogg<R: std::io::Read + std::io::Seek>(reader: R) -> Result<(Vec<Vec<f32>>, u32)> {
    let mut packet_reader = ogg::PacketReader::new(reader);
    let mut opus_decoder = None;
    let mut channels = 1;
    let mut all_data = vec![];
    while let Some(packet) = packet_reader.read_packet()? {
        let is_header = packet.data.len() >= 8 && &packet.data[0..8] == b"OpusHead";
        let is_tags = packet.data.len() >= 8 && &packet.data[0..8] == b"OpusTags";
        if is_tags {
            continue;
        }
        match (is_header, opus_decoder.as_mut()) {
            (true, Some(_)) => anyhow::bail!("multiple OpusHead packets"),
            (true, None) => {
                let header = parse_opus_header(&packet.data)?;
                channels = header.channel_count as usize;
                let channels = match header.channel_count {
                    1 => opus::Channels::Mono,
                    2 => opus::Channels::Stereo,
                    c => anyhow::bail!("unexpected number of channels {c}"),
                };
                let od = opus::Decoder::new(OPUS_SAMPLE_RATE, channels)?;
                opus_decoder = Some(od)
            }
            (false, None) => anyhow::bail!("no initial OpusHead"),
            (false, Some(od)) => {
                let nb_samples = od.get_nb_samples(&packet.data)?;
                let prev_len = all_data.len();
                all_data.resize(prev_len + nb_samples * channels, 0f32);
                let samples = od.decode_float(
                    &packet.data,
                    &mut all_data[prev_len..],
                    /* Forward Error Correction */ false,
                )?;
                all_data.resize(prev_len + samples * channels, 0f32);
            }
        }
    }
    let sample_rate = match opus_decoder.as_mut() {
        None => anyhow::bail!("no data"),
        Some(od) => od.get_sample_rate()?,
    };
    let data = match channels {
        1 => vec![all_data],
        2 => {
            let mut c0 = Vec::with_capacity(all_data.len() / 2);
            let mut c1 = Vec::with_capacity(all_data.len() / 2);
            for c in all_data.chunks(2) {
                c0.push(c[0]);
                c1.push(c[1]);
            }
            vec![c0, c1]
        }
        c => anyhow::bail!("unexpected number of channels {c}"),
    };
    Ok((data, sample_rate))
}

fn write_opus_header<W: std::io::Write>(
    w: &mut W,
    channels: u8,
    sample_rate: u32,
) -> std::io::Result<()> {
    use byteorder::WriteBytesExt;

    // https://wiki.xiph.org/OggOpus#ID_Header
    w.write_all(b"OpusHead")?;
    w.write_u8(1)?; // version
    w.write_u8(channels)?; // channel count
    w.write_u16::<byteorder::LittleEndian>(3840)?; // pre-skip
    w.write_u32::<byteorder::LittleEndian>(sample_rate)?; //  sample-rate in Hz
    w.write_i16::<byteorder::LittleEndian>(0)?; // output gain Q7.8 in dB
    w.write_u8(0)?; // channel map
    Ok(())
}

fn write_opus_tags<W: std::io::Write>(w: &mut W) -> std::io::Result<()> {
    use byteorder::WriteBytesExt;

    // https://wiki.xiph.org/OggOpus#Comment_Header
    let vendor = "sphn-pyo3";
    w.write_all(b"OpusTags")?;
    w.write_u32::<byteorder::LittleEndian>(vendor.len() as u32)?; // vendor string length
    w.write_all(vendor.as_bytes())?; // vendor string, UTF8 encoded
    w.write_u32::<byteorder::LittleEndian>(0u32)?; // number of tags
    Ok(())
}

// Opus audio is always encoded at 48kHz, this function assumes that it is the case. The
// input_sample_rate is only indicative of the sample rate of the original source (which appears in
// the opus header).
fn write_ogg_48khz<W: std::io::Write>(
    w: &mut W,
    pcm: &[f32],
    input_sample_rate: u32,
    stereo: bool,
) -> Result<()> {
    let mut pw = ogg::PacketWriter::new(w);
    let channels = if stereo { 2 } else { 1 };

    // Write the opus headers and tags
    let mut head = Vec::new();
    write_opus_header(&mut head, channels as u8, input_sample_rate)?;
    pw.write_packet(head, 42, ogg::PacketWriteEndInfo::EndPage, 0)?;
    let mut tags = Vec::new();
    write_opus_tags(&mut tags)?;
    pw.write_packet(tags, 42, ogg::PacketWriteEndInfo::EndPage, 0)?;

    // Write the actual pcm data
    let mut encoder = {
        let channels = if stereo { opus::Channels::Stereo } else { opus::Channels::Mono };
        opus::Encoder::new(OPUS_SAMPLE_RATE, channels, opus::Application::Voip)?
    };
    let mut out_encoded = vec![0u8; 50_000];

    let mut total_data = 0;
    let n_frames = pcm.len() / (channels * OPUS_ENCODER_FRAME_SIZE);
    for (frame_idx, pcm) in pcm.chunks_exact(OPUS_ENCODER_FRAME_SIZE * channels).enumerate() {
        total_data += (pcm.len() / channels) as u64;
        let size = encoder.encode_float(pcm, &mut out_encoded)?;
        let msg = out_encoded[..size].to_vec();
        let inf = if frame_idx + 1 == n_frames {
            ogg::PacketWriteEndInfo::EndPage
        } else {
            ogg::PacketWriteEndInfo::NormalPacket
        };
        pw.write_packet(msg, 42, inf, total_data)?;
    }

    Ok(())
}

pub fn write_ogg_mono<W: std::io::Write>(w: &mut W, pcm: &[f32], sample_rate: u32) -> Result<()> {
    if sample_rate == OPUS_SAMPLE_RATE {
        write_ogg_48khz(w, pcm, sample_rate, false)
    } else {
        let pcm = crate::audio::resample(pcm, sample_rate as usize, OPUS_SAMPLE_RATE as usize)?;
        write_ogg_48khz(w, &pcm, sample_rate, false)
    }
}

pub fn write_ogg_stereo<W: std::io::Write>(
    w: &mut W,
    pcm1: &[f32],
    pcm2: &[f32],
    sample_rate: u32,
) -> Result<()> {
    if sample_rate == OPUS_SAMPLE_RATE {
        let pcm = pcm1.iter().zip(pcm2.iter()).flat_map(|(s1, s2)| [*s1, *s2]).collect::<Vec<_>>();
        write_ogg_48khz(w, &pcm, sample_rate, true)
    } else {
        let pcm1 = crate::audio::resample(pcm1, sample_rate as usize, OPUS_SAMPLE_RATE as usize)?;
        let pcm2 = crate::audio::resample(pcm2, sample_rate as usize, OPUS_SAMPLE_RATE as usize)?;
        let pcm = pcm1.iter().zip(pcm2.iter()).flat_map(|(s1, s2)| [*s1, *s2]).collect::<Vec<_>>();
        write_ogg_48khz(w, &pcm, sample_rate, true)
    }
}

struct BufferStreamW(std::sync::mpsc::Sender<Vec<u8>>);

impl std::io::Write for BufferStreamW {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.0.send(buf.to_vec()).is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "opus stream writer error".to_string(),
            ));
        };
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct StreamWriter {
    pw: ogg::PacketWriter<'static, BufferStreamW>,
    encoder: opus::Encoder,
    out_encoded: Vec<u8>,
    total_data: u64,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
}

impl StreamWriter {
    pub fn new(sample_rate: u32) -> Result<Self> {
        if sample_rate != 48000 && sample_rate != 24000 {
            anyhow::bail!("sample-rate has to be 48000 or 24000, got {sample_rate}")
        }
        let encoder =
            opus::Encoder::new(sample_rate, opus::Channels::Mono, opus::Application::Voip)?;
        let (tx, rx) = std::sync::mpsc::channel();
        let mut pw = ogg::PacketWriter::new(BufferStreamW(tx));
        let out_encoded = vec![0u8; 50_000];
        let mut head = Vec::new();
        write_opus_header(&mut head, 1u8, sample_rate)?;
        pw.write_packet(head, 42, ogg::PacketWriteEndInfo::EndPage, 0)?;
        let mut tags = Vec::new();
        write_opus_tags(&mut tags)?;
        pw.write_packet(tags, 42, ogg::PacketWriteEndInfo::EndPage, 0)?;
        Ok(Self { pw, encoder, out_encoded, total_data: 0, rx })
    }

    pub fn append_pcm(&mut self, pcm: &[f32]) -> Result<()> {
        if !OPUS_ALLOWED_FRAME_SIZES.contains(&pcm.len()) {
            anyhow::bail!(
                "pcm length has to match an allowed frame size {OPUS_ALLOWED_FRAME_SIZES:?}, got {}", pcm.len()
            )
        }

        let size = self.encoder.encode_float(pcm, &mut self.out_encoded)?;
        let msg = self.out_encoded[..size].to_vec();
        self.total_data += pcm.len() as u64;
        self.pw.write_packet(msg, 42, ogg::PacketWriteEndInfo::EndPage, self.total_data)?;
        Ok(())
    }

    pub fn read_bytes(&mut self) -> Result<Vec<u8>> {
        match self.rx.try_recv() {
            Ok(data) => Ok(data),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(vec![]),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                anyhow::bail!("opus stream writer disconnected")
            }
        }
    }
}

pub struct BufferedReceiver {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    data: std::io::Cursor<Vec<u8>>,
}

pub fn seekable_channel() -> (std::sync::mpsc::Sender<Vec<u8>>, BufferedReceiver) {
    let (tx, rx) = std::sync::mpsc::channel();
    let rx = BufferedReceiver { rx, data: std::io::Cursor::new(vec![]) };
    (tx, rx)
}

impl std::io::Seek for BufferedReceiver {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.data.seek(pos)
    }
}

impl std::io::Read for BufferedReceiver {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.data.read(buf) {
            Ok(0) => {
                match self.rx.recv() {
                    Ok(data) => {
                        self.data.get_mut().extend_from_slice(&data);
                        self.read(buf)
                        // push
                    }
                    Err(_) => {
                        Ok(0) // end of stream
                    }
                }
            }
            ok_or_err => ok_or_err,
        }
    }
}

pub struct StreamReader {
    opus_tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    pcm_rx: std::sync::mpsc::Receiver<anyhow::Result<Vec<f32>>>,
}

impl StreamReader {
    pub fn new(sample_rate: u32) -> Result<Self> {
        if sample_rate != 48000 && sample_rate != 24000 {
            anyhow::bail!("sample-rate has to be 48000 or 24000, got {sample_rate}")
        }
        let mut decoder = opus::Decoder::new(sample_rate, opus::Channels::Mono)?;
        let (opus_tx, opus_rx) = seekable_channel();
        let (pcm_tx, pcm_rx) = std::sync::mpsc::channel();
        let mut pr = ogg::PacketReader::new(opus_rx);
        let mut pcm_buf = vec![0f32; 24_000 * 10];
        std::thread::spawn(move || {
            while let Ok(packet) = pr.read_packet() {
                let packet = match packet {
                    None => break,
                    Some(packet) => packet,
                };
                if packet.data.starts_with(b"OpusHead") || packet.data.starts_with(b"OpusTags") {
                    continue;
                }
                let bytes_read = decoder.decode_float(
                    &packet.data,
                    &mut pcm_buf,
                    /* Forward Error Correction */ false,
                );
                match bytes_read {
                    Err(err) => {
                        if pcm_tx.send(Err(err.into())).is_err() {
                            break;
                        };
                        break;
                    }
                    Ok(n) => {
                        if pcm_tx.send(Ok(pcm_buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self { opus_tx: Some(opus_tx), pcm_rx })
    }

    pub fn append(&mut self, data: Vec<u8>) -> Result<()> {
        match self.opus_tx.as_ref() {
            Some(opus_tx) => opus_tx.send(data)?,
            None => anyhow::bail!("StreamReader has been closed"),
        }
        Ok(())
    }

    pub fn close(&mut self) {
        // This triggers the drop of the channel if any and the thread will stop.
        self.opus_tx = None
    }

    /// Returns None at the end of the stream and an empty slice if no data is currently available.
    pub fn read_pcm(&mut self) -> Result<Option<Vec<f32>>> {
        match self.pcm_rx.try_recv() {
            Ok(data) => Ok(Some(data?)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(Some(vec![])),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Ok(None),
        }
    }
}
