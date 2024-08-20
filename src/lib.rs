mod audio;
mod opus;
mod wav;

use pyo3::prelude::*;

trait PyRes<R> {
    #[allow(unused)]
    fn w(self) -> PyResult<R>;
    fn w_f(self, annot: &std::path::Path) -> PyResult<R>;
}

impl<R, E: Into<anyhow::Error>> PyRes<R> for Result<R, E> {
    fn w(self) -> PyResult<R> {
        self.map_err(|e| pyo3::exceptions::PyValueError::new_err(e.into().to_string()))
    }
    fn w_f(self, annot: &std::path::Path) -> PyResult<R> {
        self.map_err(|e| {
            let e = e.into().to_string();
            let msg = format!("{annot:?}: {e}");
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
        let inner = audio::FileReader::new(&path).w_f(path.as_path())?;
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
            self.inner.decode(start_sec, duration_sec, false).w_f(self.path.as_path())?;
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
            self.inner.decode(start_sec, duration_sec, true).w_f(self.path.as_path())?;
        let data = numpy::PyArray2::from_vec2_bound(py, &data)?.into_py(py);
        Ok((data, unpadded_len))
    }

    /// Decodes the audio data for the whole file and return it as a two dimensional numpy array.
    fn decode_all(&mut self, py: Python) -> PyResult<PyObject> {
        let data = self.inner.decode_all().w_f(self.path.as_path())?;
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
    let mut reader = audio::FileReader::new(&filename).w_f(filename.as_path())?;
    let data = match (start_sec, duration_sec) {
        (Some(start_sec), Some(duration_sec)) => {
            reader.decode(start_sec, duration_sec, false).w_f(filename.as_path())?.0
        }
        (Some(start_sec), None) => reader.decode(start_sec, 1e9, false).w_f(filename.as_path())?.0,
        (None, Some(duration_sec)) => {
            reader.decode(0., duration_sec, false).w_f(filename.as_path())?.0
        }
        (None, None) => reader.decode_all().w_f(filename.as_path())?,
    };
    let (data, sample_rate) = match sample_rate {
        Some(out_sr) => {
            let in_sr = reader.sample_rate() as usize;
            let data = audio::resample2(&data, in_sr, out_sr as usize).w_f(filename.as_path())?;
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
    .w_f(filename.as_path())?;
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
    data: numpy::PyReadonlyArray1<f32>,
    sample_rate: u32,
) -> PyResult<()> {
    let w = std::fs::File::create(&filename).w_f(filename.as_path())?;
    let mut w = std::io::BufWriter::new(w);
    let data = data.as_array();
    match data.as_slice() {
        None => {
            let data = data.to_vec();
            wav::write(&mut w, data.as_ref(), sample_rate).w_f(filename.as_path())?
        }
        Some(data) => wav::write(&mut w, data, sample_rate).w_f(filename.as_path())?,
    }
    Ok(())
}

/// Reads the whole content of an ogg/opus encoded file.
///
/// This returns a two dimensional array as well as the sample rate.
#[pyfunction]
#[pyo3(signature = (filename))]
fn read_opus(filename: std::path::PathBuf, py: Python) -> PyResult<(PyObject, u32)> {
    let file = std::fs::File::open(&filename)?;
    let file = std::io::BufReader::new(file);
    let (data, sample_rate) = opus::read_ogg(file).w_f(filename.as_path())?;
    let data = numpy::PyArray2::from_vec2_bound(py, &data)?.into_py(py);
    Ok((data, sample_rate))
}

/// Reads bytes corresponding to an ogg/opus encoded file.
///
/// This returns a two dimensional array as well as the sample rate.
#[pyfunction]
#[pyo3(signature = (bytes))]
fn read_opus_bytes(bytes: Vec<u8>, py: Python) -> PyResult<(PyObject, u32)> {
    let bytes = std::io::Cursor::new(bytes);
    let (data, sample_rate) = opus::read_ogg(bytes).w()?;
    let data = numpy::PyArray2::from_vec2_bound(py, &data)?.into_py(py);
    Ok((data, sample_rate))
}

#[pymodule]
fn sphn(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<FileReader>()?;
    m.add_function(wrap_pyfunction!(durations, m)?)?;
    m.add_function(wrap_pyfunction!(read, m)?)?;
    m.add_function(wrap_pyfunction!(write_wav, m)?)?;
    m.add_function(wrap_pyfunction!(read_opus, m)?)?;
    m.add_function(wrap_pyfunction!(read_opus_bytes, m)?)?;
    Ok(())
}
