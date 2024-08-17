use anyhow::{Context, Result};

use symphonia::core::audio::Signal;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

pub struct FileReader {
    track_id: u32,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    format: Box<dyn symphonia::core::formats::FormatReader>,
    start_ts: u64,
    duration: Time,
    time_base: TimeBase,
    sample_rate: u32,
    channels: usize,
}

fn conv<T>(
    pcm_data: &mut [Vec<f32>],
    data: std::borrow::Cow<symphonia::core::audio::AudioBuffer<T>>,
) where
    T: symphonia::core::sample::Sample,
    f32: symphonia::core::conv::FromSample<T>,
{
    use symphonia::core::conv::FromSample;
    for (channel_index, pcm_data) in pcm_data.iter_mut().enumerate() {
        pcm_data.extend(data.chan(channel_index).iter().map(|v| f32::from_sample(*v)))
    }
}

fn conv_s<T>(
    pcm_data: &mut [Vec<f32>],
    data: std::borrow::Cow<symphonia::core::audio::AudioBuffer<T>>,
    to_skip: usize,
    samples_to_read: usize,
) -> usize
where
    T: symphonia::core::sample::Sample,
    f32: symphonia::core::conv::FromSample<T>,
{
    use symphonia::core::conv::FromSample;
    let mut remaining_to_skip = 0;
    for (channel_index, pcm_data) in pcm_data.iter_mut().enumerate() {
        let data = data.chan(channel_index);
        if to_skip < data.len() {
            let data = &data[to_skip..];
            let missing_samples = samples_to_read.saturating_sub(pcm_data.len());
            let data = &data[..usize::min(data.len(), missing_samples)];
            pcm_data.extend(data.iter().map(|v| f32::from_sample(*v)));
        } else {
            remaining_to_skip = to_skip - data.len()
        }
    }
    remaining_to_skip
}

pub trait IntoTime {
    fn into_time(self) -> Time;
}

impl IntoTime for Time {
    fn into_time(self) -> Time {
        self
    }
}

impl IntoTime for f64 {
    fn into_time(self) -> Time {
        let self_u64 = self as u64;
        Time::new(self_u64, self.fract())
    }
}

impl FileReader {
    pub fn new<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let src = std::fs::File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(src), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|v| v.to_str()) {
            hint.with_extension(extension);
        }

        // Use the default options for metadata and format readers.
        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = Default::default();

        // Probe the media source.
        let probed = symphonia::default::get_probe().format(&hint, mss, &fmt_opts, &meta_opts)?;

        // Get the instantiated format reader.
        let format = probed.format;

        // Find the first audio track with a known (decodeable) codec.
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .context("no useable codec")?;
        let time_base = track.codec_params.time_base.context("no time-base")?;
        let sample_rate = track.codec_params.sample_rate.context("no sample-rate")?;
        let n_frames = track.codec_params.n_frames.context("no n-frames")?;
        let start_ts = track.codec_params.start_ts;
        let duration = time_base.calc_time(n_frames);
        let channels = match track.codec_params.channels {
            Some(c) => c.count(),
            None => match track.codec_params.channel_layout {
                None => anyhow::bail!("no channel"),
                Some(symphonia::core::audio::Layout::Mono) => 1,
                Some(symphonia::core::audio::Layout::Stereo) => 2,
                Some(l) => anyhow::bail!("unsupported layout {l:?}"),
            },
        };

        // Use the default options for the decoder.
        let dec_opts: DecoderOptions = Default::default();

        // Create a decoder for the track.
        let decoder = symphonia::default::get_codecs().make(&track.codec_params, &dec_opts)?;

        // Store the track identifier, it will be used to filter packets.
        let track_id = track.id;
        Ok(Self { track_id, decoder, format, time_base, start_ts, duration, sample_rate, channels })
    }

    pub fn duration_sec(&self) -> f64 {
        self.duration.seconds as f64 + self.duration.frac
    }

    pub fn decode<I1: IntoTime, I2: IntoTime>(
        &mut self,
        start_time: I1,
        duration: I2,
        pad_with_zeros: bool,
    ) -> Result<(Vec<Vec<f32>>, usize)> {
        let start_time = start_time.into_time();
        let duration = duration.into_time();
        let start_ts = self.time_base.calc_timestamp(start_time);
        let samples_to_read = self.time_base.calc_timestamp(duration) as usize;
        let mut pcm_data = vec![Vec::with_capacity(samples_to_read); self.channels];
        // Somehow using Time rather than TimeStamp in the seek below doesn't seem to have much
        // effects.
        let seeked_to = self.format.seek(
            symphonia::core::formats::SeekMode::Accurate,
            symphonia::core::formats::SeekTo::TimeStamp { ts: start_ts, track_id: self.track_id },
        )?;
        self.decoder.reset();
        let mut to_skip = start_ts.saturating_sub(seeked_to.actual_ts) as usize;

        while pcm_data[0].len() < samples_to_read {
            // Get the next packet from the media format.
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(ioerr)) if ioerr.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break
                }
                Err(err) => Err(err)?,
            };
            while !self.format.metadata().is_latest() {
                self.format.metadata().pop();
            }
            if packet.track_id() != self.track_id {
                continue;
            }

            // Decode the packet into audio samples.
            let decoded = self.decoder.decode(&packet)?;
            to_skip = match decoded {
                symphonia::core::audio::AudioBufferRef::F32(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
                symphonia::core::audio::AudioBufferRef::U8(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
                symphonia::core::audio::AudioBufferRef::U16(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
                symphonia::core::audio::AudioBufferRef::U24(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
                symphonia::core::audio::AudioBufferRef::U32(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
                symphonia::core::audio::AudioBufferRef::S8(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
                symphonia::core::audio::AudioBufferRef::S16(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
                symphonia::core::audio::AudioBufferRef::S24(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
                symphonia::core::audio::AudioBufferRef::S32(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
                symphonia::core::audio::AudioBufferRef::F64(data) => {
                    conv_s(&mut pcm_data, data, to_skip, samples_to_read)
                }
            };
        }
        let unpaded_len = if pcm_data.is_empty() { 0 } else { pcm_data[0].len() };
        if pad_with_zeros && unpaded_len < samples_to_read {
            for pcm_data in pcm_data.iter_mut() {
                pcm_data.resize(samples_to_read, 0f32)
            }
        }
        Ok((pcm_data, unpaded_len))
    }

    pub fn decode_all(&mut self) -> Result<Vec<Vec<f32>>> {
        let mut pcm_data = vec![vec![]; self.channels];
        self.format.seek(
            symphonia::core::formats::SeekMode::Accurate,
            symphonia::core::formats::SeekTo::TimeStamp {
                ts: self.start_ts,
                track_id: self.track_id,
            },
        )?;
        self.decoder.reset();

        loop {
            // Get the next packet from the media format.
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(ioerr)) if ioerr.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break
                }
                Err(err) => Err(err)?,
            };
            // Consume any new metadata that has been read since the last packet.
            while !self.format.metadata().is_latest() {
                // Pop the old head of the metadata queue.
                self.format.metadata().pop();

                // Consume the new metadata at the head of the metadata queue.
            }

            if packet.track_id() != self.track_id {
                continue;
            }

            // Decode the packet into audio samples.
            let decoded = self.decoder.decode(&packet)?;
            match decoded {
                symphonia::core::audio::AudioBufferRef::F32(data) => {
                    for (channel_index, pcm_data) in pcm_data.iter_mut().enumerate() {
                        pcm_data.extend_from_slice(data.chan(channel_index))
                    }
                }
                symphonia::core::audio::AudioBufferRef::U8(data) => conv(&mut pcm_data, data),
                symphonia::core::audio::AudioBufferRef::U16(data) => conv(&mut pcm_data, data),
                symphonia::core::audio::AudioBufferRef::U24(data) => conv(&mut pcm_data, data),
                symphonia::core::audio::AudioBufferRef::U32(data) => conv(&mut pcm_data, data),
                symphonia::core::audio::AudioBufferRef::S8(data) => conv(&mut pcm_data, data),
                symphonia::core::audio::AudioBufferRef::S16(data) => conv(&mut pcm_data, data),
                symphonia::core::audio::AudioBufferRef::S24(data) => conv(&mut pcm_data, data),
                symphonia::core::audio::AudioBufferRef::S32(data) => conv(&mut pcm_data, data),
                symphonia::core::audio::AudioBufferRef::F64(data) => conv(&mut pcm_data, data),
            };
        }
        Ok(pcm_data)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }
}

pub fn resample(pcm_in: &[f32], sr_in: usize, sr_out: usize) -> anyhow::Result<Vec<f32>> {
    use rubato::Resampler;

    let mut pcm_out =
        Vec::with_capacity((pcm_in.len() as f64 * sr_out as f64 / sr_in as f64) as usize + 1024);

    let mut resampler = rubato::FftFixedInOut::<f32>::new(sr_in, sr_out, 1024, 1)?;
    let mut output_buffer = resampler.output_buffer_allocate(true);
    let mut pos_in = 0;
    while pos_in + resampler.input_frames_next() < pcm_in.len() {
        let (in_len, out_len) =
            resampler.process_into_buffer(&[&pcm_in[pos_in..]], &mut output_buffer, None)?;
        pos_in += in_len;
        pcm_out.extend_from_slice(&output_buffer[0][..out_len]);
    }

    if pos_in < pcm_in.len() {
        let (_in_len, out_len) = resampler.process_partial_into_buffer(
            Some(&[&pcm_in[pos_in..]]),
            &mut output_buffer,
            None,
        )?;
        pcm_out.extend_from_slice(&output_buffer[0][..out_len]);
    }

    Ok(pcm_out)
}

pub fn resample2(
    pcm_in: &[Vec<f32>],
    sr_in: usize,
    sr_out: usize,
) -> anyhow::Result<Vec<Vec<f32>>> {
    pcm_in
        .iter()
        .map(|data| {
            let data = resample(data, sr_in, sr_out)?;
            Ok::<_, anyhow::Error>(data)
        })
        .collect::<Result<Vec<_>, _>>()
}
