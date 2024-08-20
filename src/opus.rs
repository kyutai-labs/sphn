use anyhow::Result;

#[allow(unused)]
#[derive(Debug)]
struct OpusHeader {
    version: u8,
    channel_count: u8,
    pre_skip: u16,
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
                let od = opus::Decoder::new(header.input_sample_rate, channels)?;
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
