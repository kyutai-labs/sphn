mod audio;
mod opus;
mod wav;

use pyo3::prelude::*;

trait PyRes<R> {
    #[allow(unused)]
    fn w(self) -> PyResult<R>;
    fn w_f<P: AsRef<std::path::Path>>(self, p: P) -> PyResult<R>;
}

impl<R, E: Into<anyhow::Error>> PyRes<R> for Result<R, E> {
    fn w(self) -> PyResult<R> {
        self.map_err(|e| pyo3::exceptions::PyValueError::new_err(e.into().to_string()))
    }
    fn w_f<P: AsRef<std::path::Path>>(self, p: P) -> PyResult<R> {
        self.map_err(|e| {
            let e = e.into().to_string();
            let msg = format!("{:?}: {e}", p.as_ref());
            pyo3::exceptions::PyValueError::new_err(msg)
        })
    }
}

#[macro_export]
macro_rules! py_bail {
    ($msg:literal $(,)?) => {
        return Err(pyo3::exceptions::PyValueError::new_err(format!($msg)))
    };
    ($err:expr $(,)?) => {
        return Err(pyo3::exceptions::PyValueError::new_err(format!($err)))
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Err(pyo3::exceptions::PyValueError::new_err(format!($fmt, $($arg)*)))
    };
}

#[pyclass]
struct FileReader {
    inner: audio::FileReader,
    path: std::path::PathBuf,
}

#[pymethods]
impl FileReader {
    #[new]
    fn new(path: std::path::PathBuf) -> PyResult<Self> {
        let inner = audio::FileReader::new(&path).w_f(&path)?;
        Ok(Self { inner, path: path.to_path_buf() })
    }

    fn __str__(&self) -> String {
        format!("FileReader(path={:?})", self.path)
    }

    /// The duration of the audio stream in seconds.
    #[getter]
    fn duration_sec(&self) -> f64 {
        self.inner.duration_sec()
    }

    /// The sample rate as an int.
    #[getter]
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    /// The number of channels.
    #[getter]
    fn channels(&self) -> usize {
        self.inner.channels()
    }

    /// Decodes the audio data from `start_sec` to `start_sec + duration_sec` and return the PCM
    /// data as a two dimensional numpy array. The first dimension is the channel, the second one
    /// is time.
    /// If the end of the file is reached, the decoding stops and the already decoded data is
    /// returned.
    fn decode(&mut self, start_sec: f64, duration_sec: f64, py: Python) -> PyResult<PyObject> {
        let (data, _unpadded_len) =
            self.inner.decode(start_sec, duration_sec, false).w_f(&self.path)?;
        Ok(numpy::PyArray2::from_vec2_bound(py, &data)?.into_py(py))
    }

    /// Decodes the audio data from `start_sec` to `start_sec + duration_sec` and return the PCM
    /// data as a two dimensional numpy array. The first dimension is the channel, the second one
    /// is time.
    /// If the end of the file is reached, the array is padded with zeros so that its length is
    /// still matching `duration_sec`.
    fn decode_with_padding(
        &mut self,
        start_sec: f64,
        duration_sec: f64,
        py: Python,
    ) -> PyResult<(PyObject, usize)> {
        let (data, unpadded_len) =
            self.inner.decode(start_sec, duration_sec, true).w_f(&self.path)?;
        let data = numpy::PyArray2::from_vec2_bound(py, &data)?.into_py(py);
        Ok((data, unpadded_len))
    }

    /// Decodes the audio data for the whole file and return it as a two dimensional numpy array.
    fn decode_all(&mut self, py: Python) -> PyResult<PyObject> {
        let data = self.inner.decode_all().w_f(&self.path)?;
        Ok(numpy::PyArray2::from_vec2_bound(py, &data)?.into_py(py))
    }
}

/// Returns the durations for the audio files passed as input.
///
/// The input argument is a list of filenames. For each of these files, the duration in seconds is
/// returned as a float, None is returned if the files cannot be open or properly read.
#[pyfunction]
fn durations(filenames: Vec<std::path::PathBuf>) -> Vec<Option<f64>> {
    use rayon::prelude::*;
    filenames
        .par_iter()
        .map(|filename| {
            let mut reader = audio::FileReader::new(filename).ok()?;
            // Try to read a small portion of the file to check that it works.
            let (_data, _unpadded_len) = reader.decode(0., 0.1, false).ok()?;
            Some(reader.duration_sec())
        })
        .collect()
}

/// Reads the content of an audio file and returns it as a numpy array.
///
/// The input argument is a filename. Its content is decoded the audio data for the whole file and
/// return it as a two dimensional numpy array as well as the sample rate.
#[pyfunction]
#[pyo3(signature = (filename, start_sec=None, duration_sec=None, sample_rate=None))]
fn read(
    filename: std::path::PathBuf,
    start_sec: Option<f64>,
    duration_sec: Option<f64>,
    sample_rate: Option<u32>,
) -> PyResult<(PyObject, u32)> {
    let mut reader = audio::FileReader::new(&filename).w_f(&filename)?;
    let data = match (start_sec, duration_sec) {
        (Some(start_sec), Some(duration_sec)) => {
            reader.decode(start_sec, duration_sec, false).w_f(&filename)?.0
        }
        (Some(start_sec), None) => reader.decode(start_sec, 1e9, false).w_f(&filename)?.0,
        (None, Some(duration_sec)) => reader.decode(0., duration_sec, false).w_f(&filename)?.0,
        (None, None) => reader.decode_all().w_f(&filename)?,
    };
    let (data, sample_rate) = match sample_rate {
        Some(out_sr) => {
            let in_sr = reader.sample_rate() as usize;
            let data = audio::resample2(&data, in_sr, out_sr as usize).w_f(&filename)?;
            (data, out_sr)
        }
        None => {
            let sample_rate = reader.sample_rate();
            (data, sample_rate)
        }
    };
    let data = Python::with_gil(|py| {
        Ok::<_, PyErr>(numpy::PyArray2::from_vec2_bound(py, &data)?.into_py(py))
    })
    .w_f(&filename)?;
    Ok((data, sample_rate))
}

/// Writes an audio file using the wav format based on pcm data from a numpy array.
///
/// This only supports a single channel at the moment so the input array data is expected to have a
/// single dimension.
#[pyfunction]
#[pyo3(signature = (filename, data, sample_rate))]
fn write_wav(
    filename: std::path::PathBuf,
    data: numpy::PyReadonlyArrayDyn<f32>,
    sample_rate: u32,
) -> PyResult<()> {
    let w = std::fs::File::create(&filename).w_f(&filename)?;
    let mut w = std::io::BufWriter::new(w);
    let data = data.as_array();
    match data.ndim() {
        1 => {
            let data = data.into_dimensionality::<numpy::Ix1>().w()?;
            let data = to_cow(&data);
            wav::write_mono(&mut w, &data, sample_rate).w_f(&filename)?;
        }
        2 => {
            let data = data.into_dimensionality::<numpy::Ix2>().w()?;
            match data.shape() {
                [1, l] => {
                    let data = data.into_shape((*l,)).w()?;
                    let data = to_cow(&data);
                    wav::write_mono(&mut w, &data, sample_rate).w_f(&filename)?;
                }
                [2, l] => {
                    let data = data.into_shape((2 * *l,)).w()?;
                    let data = to_cow(&data);
                    let (pcm1, pcm2) = (&data[..*l], &data[*l..]);
                    let data = pcm1
                        .iter()
                        .zip(pcm2.iter())
                        .flat_map(|(s1, s2)| [*s1, *s2])
                        .collect::<Vec<_>>();
                    println!("{:?}", &data[..20]);
                    wav::write_stereo(&mut w, &data, sample_rate).w_f(&filename)?
                }
                _ => py_bail!("expected one or two channels, got shape {:?}", data.shape()),
            }
        }
        _ => py_bail!("expected one or two dimensions, got shape {:?}", data.shape()),
    }
    Ok(())
}

/// Writes an opus file containing the input pcm data.
///
/// Opus content is always encoded at 48kHz so the pcm data is resampled if sample_rate is
/// different from 48000.
#[pyfunction]
#[pyo3(signature = (filename, data, sample_rate))]
fn write_opus(
    filename: std::path::PathBuf,
    data: numpy::PyReadonlyArrayDyn<f32>,
    sample_rate: u32,
) -> PyResult<()> {
    let write_mono = |mut w: std::io::BufWriter<std::fs::File>,
                      data: numpy::ndarray::ArrayView1<f32>| {
        let data = to_cow(&data);
        opus::write_ogg_mono(&mut w, &data, sample_rate).w_f(&filename)
    };

    let w = std::fs::File::create(&filename).w_f(&filename)?;
    let mut w = std::io::BufWriter::new(w);
    let data = data.as_array();
    match data.ndim() {
        1 => {
            let data = data.into_dimensionality::<numpy::Ix1>().w()?;
            write_mono(w, data)?
        }
        2 => {
            let data = data.into_dimensionality::<numpy::Ix2>().w()?;
            match data.shape() {
                [1, l] => {
                    let data = data.into_shape((*l,)).w()?;
                    write_mono(w, data)?
                }
                [2, l] => {
                    let data = data.into_shape((*l * 2,)).w()?;
                    let data = to_cow(&data);
                    let (pcm1, pcm2) = (&data[..*l], &data[*l..]);
                    opus::write_ogg_stereo(&mut w, pcm1, pcm2, sample_rate).w_f(&filename)?
                }
                _ => py_bail!("expected one or two channels, got shape {:?}", data.shape()),
            }
        }
        _ => py_bail!("expected one or two dimensions, got shape {:?}", data.shape()),
    }
    Ok(())
}

fn to_cow<'a, T: ToOwned + Clone>(
    data: &'a numpy::ndarray::ArrayView1<T>,
) -> std::borrow::Cow<'a, [T]>
where
    [T]: ToOwned<Owned = Vec<T>>,
{
    match data.as_slice() {
        None => std::borrow::Cow::Owned(data.to_vec()),
        Some(data) => std::borrow::Cow::Borrowed(data),
    }
}

/// Resamples some pcm data.
#[pyfunction]
#[pyo3(signature = (pcm, src_sample_rate, dst_sample_rate))]
fn resample(
    pcm: numpy::PyReadonlyArrayDyn<f32>,
    src_sample_rate: usize,
    dst_sample_rate: usize,
) -> PyResult<PyObject> {
    let pcm = pcm.as_array();
    match pcm.ndim() {
        1 => {
            let pcm = pcm.into_dimensionality::<numpy::Ix1>().w()?;
            let pcm = to_cow(&pcm);
            let pcm = audio::resample(&pcm[..], src_sample_rate, dst_sample_rate).w()?;
            Python::with_gil(|py| {
                Ok::<_, PyErr>(numpy::PyArray1::from_vec_bound(py, pcm).into_py(py))
            })
        }
        2 => {
            let pcm = pcm.into_dimensionality::<numpy::Ix2>().w()?;
            let (channels, l) = pcm.dim();
            let pcm = pcm.into_shape((channels * l,)).w()?;
            let pcm = to_cow(&pcm)
                .chunks(l)
                .map(|pcm| audio::resample(pcm, src_sample_rate, dst_sample_rate))
                .collect::<anyhow::Result<Vec<_>>>()
                .w()?;
            Python::with_gil(|py| {
                Ok::<_, PyErr>(numpy::PyArray2::from_vec2_bound(py, &pcm)?.into_py(py))
            })
        }
        _ => py_bail!("expected one or two dimensions, got shape {:?}", pcm.shape()),
    }
}

/// Reads the whole content of an ogg/opus encoded file.
///
/// This returns a two dimensional array as well as the sample rate. Currently all opus audio is
/// encoded at 48kHz so this value is always returned.
#[pyfunction]
#[pyo3(signature = (filename))]
fn read_opus(filename: std::path::PathBuf, py: Python) -> PyResult<(PyObject, u32)> {
    let file = std::fs::File::open(&filename)?;
    let file = std::io::BufReader::new(file);
    let (data, sample_rate) = opus::read_ogg(file).w_f(&filename)?;
    let data = numpy::PyArray2::from_vec2_bound(py, &data)?.into_py(py);
    Ok((data, sample_rate))
}

/// Reads bytes corresponding to an ogg/opus encoded file.
///
/// This returns a two dimensional array as well as the sample rate. Currently all opus audio is
/// encoded at 48kHz so this value is always returned.
#[pyfunction]
#[pyo3(signature = (bytes))]
fn read_opus_bytes(bytes: Vec<u8>, py: Python) -> PyResult<(PyObject, u32)> {
    let bytes = std::io::Cursor::new(bytes);
    let (data, sample_rate) = opus::read_ogg(bytes).w()?;
    let data = numpy::PyArray2::from_vec2_bound(py, &data)?.into_py(py);
    Ok((data, sample_rate))
}

#[pyclass]
struct OpusStreamWriter {
    inner: opus::StreamWriter,
    sample_rate: u32,
}

#[pymethods]
impl OpusStreamWriter {
    #[new]
    fn new(sample_rate: u32) -> PyResult<Self> {
        let inner = opus::StreamWriter::new(sample_rate).w()?;
        Ok(Self { inner, sample_rate })
    }

    fn __str__(&self) -> String {
        format!("OpusStreamWriter(sample_rate={})", self.sample_rate)
    }

    /// Appends one frame of pcm data to the stream. The data should be a 1d numpy array using
    /// float values, the number of elements must be an allowed frame size, e.g. 960 or 1920.
    fn append_pcm(&mut self, pcm: numpy::PyReadonlyArray1<f32>) -> PyResult<()> {
        let pcm = pcm.as_array();
        let pcm = to_cow(&pcm);
        self.inner.append_pcm(&pcm).w()?;
        Ok(())
    }

    /// Gets the pending opus bytes from the stream. An empty bytes object is returned if no data
    /// is currently available.
    fn read_bytes(&mut self) -> PyResult<PyObject> {
        let bytes = self.inner.read_bytes().w()?;
        let bytes = Python::with_gil(|py| pyo3::types::PyBytes::new_bound(py, &bytes).into_py(py));
        Ok(bytes)
    }
}

#[pyclass]
struct OpusStreamReader {
    inner: opus::StreamReader,
    sample_rate: u32,
}

#[pymethods]
impl OpusStreamReader {
    #[new]
    fn new(sample_rate: u32) -> PyResult<Self> {
        let inner = opus::StreamReader::new(sample_rate).w()?;
        Ok(Self { inner, sample_rate })
    }

    fn __str__(&self) -> String {
        format!("OpusStreamReader(sample_rate={})", self.sample_rate)
    }

    /// Writes some ogg/opus bytes to the current stream.
    fn append_bytes(&mut self, data: &[u8]) -> PyResult<()> {
        self.inner.append(data.to_vec()).w()
    }

    // TODO(laurent): maybe we should also have a pyo3_async api here.
    /// Gets the pcm data decoded by the stream, this returns a 1d numpy array or None if the
    /// stream has been closed. The array is empty if no data is currently available.
    fn read_pcm(&mut self) -> PyResult<PyObject> {
        let pcm_data = self.inner.read_pcm().w()?;
        Python::with_gil(|py| match pcm_data {
            None => Ok(py.None()),
            Some(data) => {
                let data = numpy::PyArray1::from_vec_bound(py, data.to_vec()).into_py(py);
                Ok(data)
            }
        })
    }

    /// Closes the stream, this results in the worker thread exiting and the follow up
    /// calls to `read_pcm` will return None once all the pcm data has been returned.
    fn close(&mut self) {
        self.inner.close()
    }
}

#[pymodule]
fn sphn(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<FileReader>()?;
    m.add_class::<OpusStreamReader>()?;
    m.add_class::<OpusStreamWriter>()?;
    m.add_function(wrap_pyfunction!(durations, m)?)?;
    m.add_function(wrap_pyfunction!(read, m)?)?;
    m.add_function(wrap_pyfunction!(write_wav, m)?)?;
    m.add_function(wrap_pyfunction!(read_opus, m)?)?;
    m.add_function(wrap_pyfunction!(read_opus_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(write_opus, m)?)?;
    m.add_function(wrap_pyfunction!(resample, m)?)?;
    Ok(())
}
