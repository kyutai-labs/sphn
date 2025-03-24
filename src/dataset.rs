use crate::{audio, par_map, py_bail, PyRes};
use pyo3::prelude::*;
use rand::{Rng, SeedableRng};
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PathWithDuration {
    path: String,
    duration: f64,
}

type Paths = Arc<Vec<PathWithDuration>>;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum OnError {
    Raise,
    Log,
    Ignore,
}

struct Sample {
    sample_index: u64,
    file_index: usize,
    start_time: f64,
    sample_rate: usize,
    unpadded_len: usize,
    data: anyhow::Result<Vec<Vec<f32>>>,
    gen_duration: f64,
}

impl Sample {
    fn into_dict(
        self,
        py: Python<'_>,
        on_error: OnError,
        path: &str,
    ) -> PyResult<Option<PyObject>> {
        let data = match self.data {
            Ok(sample) => sample,
            Err(err) => match on_error {
                OnError::Raise => py_bail!("{path}: {err:?}"),
                OnError::Log => {
                    eprintln!("{path}: {err:?}");
                    return Ok(None);
                }
                OnError::Ignore => {
                    return Ok(None);
                }
            },
        };
        let dict = pyo3::types::PyDict::new(py);
        let path = pyo3::types::PyString::intern(py, path);
        dict.set_item("sample_index", self.sample_index)?;
        dict.set_item("file_index", self.file_index)?;
        dict.set_item("path", path)?;
        dict.set_item("start_time_sec", self.start_time)?;
        dict.set_item("sample_rate", self.sample_rate)?;
        dict.set_item("unpadded_len", self.unpadded_len)?;
        dict.set_item("gen_duration_sec", self.gen_duration)?;
        dict.set_item::<_, PyObject>(
            "data",
            numpy::PyArray2::from_vec2(py, &data)?.into_any().unbind(),
        )?;
        Ok(Some(dict.into_any().unbind()))
    }
}

enum SampleOrObject {
    Sample(Sample),
    Object(PyResult<Option<PyObject>>),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum IterOrder {
    Sequential,
    RandomWithReplacement,
    RandomNoReplacement,
}

#[pyclass]
#[derive(Clone)]
pub struct DatasetReader {
    paths: Paths,
    duration_sec: f64,
    iter_order: IterOrder,
    seed: u64,
    skip: u64,
    num_threads: usize,
    on_error: OnError,
    step_by: u64,
    pad_last_segment: bool,
    sample_rate: Option<usize>,
    channel_len_per_thread: usize,
    f: Option<Arc<PyObject>>,
}

#[pymethods]
impl DatasetReader {
    #[allow(clippy::too_many_arguments)]
    /// Creates a reader object on a list of pairs `(filename, duration_in_seconds)`.
    #[pyo3(signature = (paths, *, duration_sec, channel_len_per_thread=1, pad_last_segment=false, on_error=None, sample_rate=None, num_threads=None, f=None))]
    #[new]
    fn new(
        paths: Vec<(String, f64)>,
        duration_sec: f64,
        channel_len_per_thread: usize,
        pad_last_segment: bool,
        on_error: Option<&str>,
        sample_rate: Option<usize>,
        num_threads: Option<usize>,
        f: Option<PyObject>,
    ) -> PyResult<Self> {
        let on_error = match on_error {
            Some("raise") => OnError::Raise,
            Some("log") | None => OnError::Log,
            Some("ignore") => OnError::Ignore,
            Some(on_error) => py_bail!("unknown on_error '{on_error}'"),
        };
        let paths: Vec<PathWithDuration> = paths
            .iter()
            .map(|(path, duration)| PathWithDuration {
                path: path.to_string(),
                duration: *duration,
            })
            .collect();
        Ok(Self {
            paths: Arc::new(paths),
            duration_sec,
            iter_order: IterOrder::Sequential,
            seed: 1337,
            skip: 0,
            on_error,
            num_threads: num_threads.unwrap_or_else(rayon::current_num_threads),
            step_by: 1,
            sample_rate,
            pad_last_segment,
            channel_len_per_thread,
            f: f.map(Arc::new),
        })
    }

    /// Sequential reading.
    #[pyo3(signature = (*, skip=0, step_by=1))]
    fn seq(&self, skip: u64, step_by: u64) -> Self {
        Self {
            paths: self.paths.clone(),
            duration_sec: self.duration_sec,
            iter_order: IterOrder::Sequential,
            seed: self.seed,
            skip,
            on_error: self.on_error,
            num_threads: self.num_threads,
            step_by,
            sample_rate: self.sample_rate,
            pad_last_segment: self.pad_last_segment,
            channel_len_per_thread: self.channel_len_per_thread,
            f: self.f.clone(),
        }
    }

    /// Randomized reading.
    #[pyo3(signature = (*, with_replacement=false, seed=299792458, skip=0, step_by=1))]
    fn shuffle(&self, with_replacement: bool, seed: u64, skip: u64, step_by: u64) -> Self {
        let iter_order = if with_replacement {
            IterOrder::RandomWithReplacement
        } else {
            IterOrder::RandomNoReplacement
        };
        Self {
            paths: self.paths.clone(),
            duration_sec: self.duration_sec,
            iter_order,
            seed,
            skip,
            on_error: self.on_error,
            num_threads: self.num_threads,
            step_by,
            sample_rate: self.sample_rate,
            pad_last_segment: self.pad_last_segment,
            channel_len_per_thread: self.channel_len_per_thread,
            f: self.f.clone(),
        }
    }

    #[pyo3(signature = (num_threads))]
    fn num_threads(&self, num_threads: usize) -> Self {
        let mut s = self.clone();
        s.num_threads = num_threads;
        s
    }

    #[pyo3(signature = (p))]
    fn pad_last_segment(&self, p: bool) -> Self {
        let mut s = self.clone();
        s.pad_last_segment = p;
        s
    }

    #[pyo3(signature = (on_error))]
    fn on_error(&self, on_error: &str) -> PyResult<Self> {
        let on_error = match on_error {
            "raise" => OnError::Raise,
            "log" => OnError::Log,
            "ignore" => OnError::Ignore,
            _ => py_bail!("unknown on_error '{on_error}'"),
        };
        let mut s = self.clone();
        s.on_error = on_error;
        Ok(s)
    }

    fn __iter__(&self, py: Python) -> PyResult<PyObject> {
        // Import the threading module from the "main" thread to avoid the dreadful
        // "assert tlock.locked()" errors.
        let _m = py.import("threading")?;

        match self.iter_order {
            IterOrder::Sequential => {
                let iter = DatasetIter::new_shuffle(
                    &self.paths,
                    None,
                    self.skip,
                    self.step_by,
                    self.duration_sec,
                    self.on_error,
                    self.num_threads,
                    self.pad_last_segment,
                    self.channel_len_per_thread,
                    self.sample_rate,
                    self.f.clone(),
                )?;
                Ok(iter.into_pyobject(py).w()?.into_any().unbind())
            }
            IterOrder::RandomWithReplacement => {
                let iter = DatasetIter::new_random(
                    &self.paths,
                    self.seed,
                    self.skip,
                    self.step_by,
                    self.duration_sec,
                    self.on_error,
                    self.num_threads,
                    self.pad_last_segment,
                    self.channel_len_per_thread,
                    self.sample_rate,
                    self.f.clone(),
                )?;
                Ok(iter.into_pyobject(py).w()?.into_any().unbind())
            }
            IterOrder::RandomNoReplacement => {
                let iter = DatasetIter::new_shuffle(
                    &self.paths,
                    Some(self.seed),
                    self.skip,
                    self.step_by,
                    self.duration_sec,
                    self.on_error,
                    self.num_threads,
                    self.pad_last_segment,
                    self.channel_len_per_thread,
                    self.sample_rate,
                    self.f.clone(),
                )?;
                Ok(iter.into_pyobject(py).w()?.into_any().unbind())
            }
        }
    }
}

/// Creates a reader object from a jsonl file.
#[allow(clippy::too_many_arguments)]
#[pyfunction(signature = (jsonl, *, duration_sec, channel_len_per_thread=1, pad_last_segment=false, on_error=None, sample_rate=None, num_threads=None, f=None))]
pub fn dataset_jsonl(
    jsonl: String,
    duration_sec: f64,
    channel_len_per_thread: usize,
    pad_last_segment: bool,
    on_error: Option<&str>,
    sample_rate: Option<usize>,
    num_threads: Option<usize>,
    f: Option<PyObject>,
) -> PyResult<DatasetReader> {
    use std::io::BufRead;

    let on_error = match on_error {
        Some("raise") => OnError::Raise,
        Some("log") | None => OnError::Log,
        Some("ignore") => OnError::Ignore,
        Some(on_error) => py_bail!("unknown on_error '{on_error}'"),
    };
    let file = std::io::BufReader::new(std::fs::File::open(jsonl)?);
    let mut paths = vec![];
    for line in file.lines() {
        let line = line?;
        let path: PathWithDuration = serde_json::from_str(line.as_str()).w()?;
        paths.push(path)
    }
    Ok(DatasetReader {
        paths: Arc::new(paths),
        duration_sec,
        iter_order: IterOrder::Sequential,
        seed: 1337,
        skip: 0,
        on_error,
        num_threads: num_threads.unwrap_or_else(rayon::current_num_threads),
        step_by: 1,
        pad_last_segment,
        sample_rate,
        channel_len_per_thread,
        f: f.map(Arc::new),
    })
}

#[allow(unused)]
#[pyclass]
pub struct DatasetIter {
    paths: Paths,
    pm: par_map::ParMap<SampleOrObject>,
    on_error: OnError,
}

#[derive(Clone)]
struct RngWithStep {
    rng: rand::rngs::StdRng,
    step_by: u64,
    index: u64,
}

impl RngWithStep {
    fn new(seed: u64, skip: u64, step_by: u64) -> Self {
        let rng = rand::rngs::StdRng::seed_from_u64(seed);
        let mut slf = Self { rng, index: 0, step_by };
        slf.skip(skip);
        slf
    }

    // Note that step_by is *not* applied to the skip value.
    fn skip(&mut self, skip: u64) {
        for _i in 0..skip {
            self.rng.gen_range(0.0..1.0);
            self.rng.gen_range(0.0..1.0);
            self.index += 1;
        }
    }

    fn next(&mut self) -> (u64, f64, f64) {
        for _i in 0..self.step_by {
            self.rng.gen_range(0.0..1.0);
            self.rng.gen_range(0.0..1.0);
            self.index += 1;
        }
        let index = self.index;
        let file_index = self.rng.gen_range(0.0..1.0);
        let start_time = self.rng.gen_range(0.0..1.0);
        self.index += 1;
        (index, file_index, start_time)
    }
}

impl DatasetIter {
    #[allow(clippy::too_many_arguments)]
    fn new_random(
        paths: &Paths,
        seed: u64,
        skip: u64,
        step_by: u64,
        duration_sec: f64,
        on_error: OnError,
        num_threads: usize,
        pad_last_segment: bool,
        channel_len_per_thread: usize,
        target_sample_rate: Option<usize>,
        f: Option<Arc<PyObject>>,
    ) -> PyResult<Self> {
        let sum_durations: f64 = paths.iter().map(|p| p.duration).sum();
        if sum_durations < 1e-5 {
            py_bail!("sum of durations is too small")
        }
        // This performs a bit of a brute-force multinomial sampling using binary search.
        let cumulative_prs = paths
            .iter()
            .scan(0.0, |acc, path| {
                *acc += path.duration / sum_durations;
                Some(*acc)
            })
            .collect::<Vec<f64>>();
        let rng = RngWithStep::new(seed, skip, step_by * num_threads as u64);
        let pm = {
            let paths = paths.clone();
            let f = f.clone();
            par_map::par_range(
                None,
                num_threads,
                channel_len_per_thread,
                move |thread_idx| {
                    let mut rng = rng.clone();
                    rng.skip(step_by * thread_idx as u64);
                    rng
                },
                move |rng| {
                    let now = std::time::Instant::now();
                    let (sample_index, file_index, start_time_1) = rng.next();
                    // [partition_point] returns the first element for which the predicate is
                    // false.
                    let file_index = cumulative_prs.partition_point(|&v| v < file_index);
                    let file_index = usize::min(file_index, cumulative_prs.len());

                    let (data, start_time, sample_rate) = 'data: {
                        let metadata = match std::fs::metadata(&paths[file_index].path) {
                            Ok(md) => md,
                            Err(err) => break 'data (Err(err.into()), 0., 0),
                        };
                        if metadata.len() == 0 {
                            break 'data (Err(anyhow::Error::msg("empty file")), 0., 0);
                        }
                        let mut reader = match audio::FileReader::new(&paths[file_index].path) {
                            Ok(reader) => reader,
                            Err(err) => break 'data (Err(err), 0., 0),
                        };
                        let left_in_reader = reader.duration_sec();
                        if left_in_reader <= duration_sec {
                            let err = Err(anyhow::format_err!(
                                "file is too small {}",
                                reader.duration_sec()
                            ));
                            break 'data (err, 0., 0);
                        }
                        let start_time = if pad_last_segment {
                            start_time_1 * left_in_reader
                        } else {
                            start_time_1 * (left_in_reader - duration_sec)
                        };
                        let (data, unpadded_len) =
                            match reader.decode(start_time, duration_sec, pad_last_segment) {
                                Ok(data) => data,
                                Err(err) => break 'data (Err(err), 0., 0),
                            };
                        let sample_rate = reader.sample_rate() as usize;
                        match target_sample_rate {
                            None => (Ok((data, unpadded_len)), start_time, sample_rate),
                            Some(target_sample_rate) => {
                                if target_sample_rate != sample_rate {
                                    let is_unpadded = unpadded_len == data[0].len();
                                    let data =
                                        audio::resample2(&data, sample_rate, target_sample_rate);
                                    match data {
                                        Ok(data) => {
                                            let unpadded_len = if is_unpadded {
                                                data[0].len()
                                            } else {
                                                unpadded_len * target_sample_rate / sample_rate
                                            };
                                            (
                                                Ok((data, unpadded_len)),
                                                start_time,
                                                target_sample_rate,
                                            )
                                        }
                                        Err(err) => break 'data (Err(err), 0., 0),
                                    }
                                } else {
                                    (Ok((data, unpadded_len)), start_time, sample_rate)
                                }
                            }
                        }
                    };
                    let unpadded_len = data.as_ref().map_or(0, |d| d.1);
                    let data = data.map(|d| d.0);
                    let sample = Sample {
                        sample_index,
                        file_index,
                        start_time,
                        sample_rate,
                        data,
                        unpadded_len,
                        gen_duration: now.elapsed().as_secs_f64(),
                    };
                    match f.as_ref() {
                        None => SampleOrObject::Sample(sample),
                        Some(f) => {
                            let f = f.clone();
                            Python::with_gil(|py| {
                                let v = sample.into_dict(py, on_error, &paths[file_index].path);
                                let v = match v {
                                    Ok(None) | Err(_) => v,
                                    Ok(Some(v)) => f.call1(py, (v,)).map(Some),
                                };
                                SampleOrObject::Object(v)
                            })
                        }
                    }
                },
            )
        };
        Ok(Self { paths: paths.clone(), pm, on_error })
    }

    #[allow(clippy::too_many_arguments)]
    fn new_shuffle(
        paths: &Paths,
        seed: Option<u64>,
        skip: u64,
        step_by: u64,
        duration_sec: f64,
        on_error: OnError,
        num_threads: usize,
        pad_last_segment: bool,
        channel_len_per_thread: usize,
        target_sample_rate: Option<usize>,
        f: Option<Arc<PyObject>>,
    ) -> PyResult<Self> {
        use rand::seq::SliceRandom;

        // For a million hours of audio with duration set to 30s, this would contain 120m elements.
        let mut segments: Vec<_> = paths
            .iter()
            .enumerate()
            .map(|(path_index, path_with_d)| {
                let mut segments = Vec::new();
                let mut start_ts = 0f64;
                while start_ts + duration_sec < path_with_d.duration {
                    segments.push((path_index as u32, start_ts as f32));
                    start_ts += duration_sec;
                }
                if pad_last_segment && start_ts < path_with_d.duration {
                    segments.push((path_index as u32, start_ts as f32));
                }
                Ok(segments)
            })
            .collect::<anyhow::Result<Vec<_>>>()
            .w()?
            .concat();
        if let Some(seed) = seed {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            segments.shuffle(&mut rng);
        }
        if skip > 0 {
            segments.drain(0..skip as usize);
        }
        let segments = if step_by > 1 {
            segments.into_iter().step_by(step_by as usize).collect::<Vec<_>>()
        } else {
            segments
        };
        let pm = {
            let paths = paths.clone();
            par_map::par_map(
                segments,
                num_threads,
                channel_len_per_thread,
                move |segment_index, (file_index, start_time)| {
                    let now = std::time::Instant::now();
                    let file_index = *file_index as usize;
                    let start_time = *start_time as f64;
                    let (data, sample_rate, unpadded_len) = 'sample: {
                        let mut reader = match audio::FileReader::new(&paths[file_index].path) {
                            Ok(reader) => reader,
                            Err(err) => break 'sample (Err(err), 0, 0),
                        };
                        let (data, unpadded_len) =
                            match reader.decode(start_time, duration_sec, pad_last_segment) {
                                Ok(data) => data,
                                Err(err) => break 'sample (Err(err), 0, 0),
                            };
                        let sample_rate = reader.sample_rate() as usize;
                        match target_sample_rate {
                            None => (Ok(data), sample_rate, unpadded_len),
                            Some(target_sample_rate) => {
                                if target_sample_rate != sample_rate {
                                    let is_unpadded = unpadded_len == data[0].len();
                                    let data =
                                        audio::resample2(&data, sample_rate, target_sample_rate);
                                    match data {
                                        Ok(data) => {
                                            let unpadded_len = if is_unpadded {
                                                data[0].len()
                                            } else {
                                                unpadded_len * target_sample_rate / sample_rate
                                            };
                                            (Ok(data), target_sample_rate, unpadded_len)
                                        }
                                        Err(err) => break 'sample (Err(err), 0, 0),
                                    }
                                } else {
                                    (Ok(data), sample_rate, unpadded_len)
                                }
                            }
                        }
                    };
                    let sample = Sample {
                        sample_index: segment_index as u64 * step_by + skip,
                        file_index,
                        start_time,
                        sample_rate,
                        data,
                        unpadded_len,
                        gen_duration: now.elapsed().as_secs_f64(),
                    };
                    match f.as_ref() {
                        None => SampleOrObject::Sample(sample),
                        Some(f) => {
                            let f = f.clone();
                            Python::with_gil(|py| {
                                let v = sample.into_dict(py, on_error, &paths[file_index].path);
                                let v = match v {
                                    Ok(None) | Err(_) => v,
                                    Ok(Some(v)) => f.call1(py, (v,)).map(Some),
                                };
                                SampleOrObject::Object(v)
                            })
                        }
                    }
                },
            )
        };
        Ok(Self { paths: paths.clone(), pm, on_error })
    }
}

#[pymethods]
impl DatasetIter {
    fn buffered_lens(&self) -> Vec<usize> {
        self.pm.buffered_lens()
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python) -> PyResult<Option<PyObject>> {
        loop {
            let sample = py.allow_threads(|| self.pm.next());
            let sample = match sample {
                Some(sample) => sample,
                None => return Ok(None),
            };
            let sample = match sample {
                SampleOrObject::Sample(sample) => {
                    let file_index = sample.file_index;
                    sample.into_dict(py, self.on_error, &self.paths[file_index].path)
                }
                SampleOrObject::Object(sample) => sample,
            };
            let sample = match sample? {
                None => {
                    py.check_signals()?;
                    continue;
                }
                Some(sample) => sample,
            };
            return Ok(Some(sample));
        }
    }
}
