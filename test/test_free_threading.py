"""Free-threading tests for sphn.

These are plain `test_*` functions, so they run with `pytest test/test_free_threading.py` or
directly with `python test/test_free_threading.py`. Unlike basic.py and stream.py they do not need
any audio file, the test signals are generated on the fly.

On a free-threaded build (3.13t/3.14t) this checks that importing sphn does not re-enable the GIL
and hammers the extension from several threads. On a regular build only the thread tests are
meaningful, the GIL tests are skipped.
"""

import os
import re
import subprocess
import sys
import sysconfig
import tempfile
import threading
import unittest

import numpy as np
import sphn

N_THREADS = 8
BORROW_ERROR = re.compile(r"Already (mutably )?borrowed")


def is_free_threaded():
    return bool(sysconfig.get_config_var("Py_GIL_DISABLED"))


def tone(freq, sr, dur=1.0):
    t = np.arange(int(sr * dur)) / sr
    return (0.5 * np.sin(2 * np.pi * freq * t)).astype(np.float32)


def peak_freq(pcm, sr):
    # Dominant frequency, this is robust to the codec delay/padding of opus and to resampling.
    spectrum = np.abs(np.fft.rfft(pcm))
    return np.fft.rfftfreq(len(pcm), 1 / sr)[np.argmax(spectrum)]


def run_threads(fn):
    """Runs fn(thread_index) on N_THREADS threads started together, fails on any exception."""
    n_threads = N_THREADS
    barrier = threading.Barrier(n_threads)
    errors = []

    def worker(idx):
        try:
            barrier.wait()
            fn(idx)
        except BaseException as e:  # noqa: B902
            errors.append((idx, repr(e)))

    threads = [threading.Thread(target=worker, args=(i,)) for i in range(n_threads)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors, errors


def check_gil_off(code):
    if not is_free_threaded():
        raise unittest.SkipTest("not a free-threaded build")
    if os.environ.get("PYTHON_GIL") == "1":
        raise unittest.SkipTest("PYTHON_GIL=1 forces the GIL")
    env = dict(os.environ)
    env.pop("PYTHON_GIL", None)
    # The warning emitted when importing an extension module that did not declare gil_used=false
    # is a RuntimeWarning, turn it into an error.
    cmd = [sys.executable, "-W", "error::RuntimeWarning", "-c", code]
    res = subprocess.run(cmd, capture_output=True, text=True, env=env)
    assert res.returncode == 0, res.stderr
    assert res.stdout.strip() == "False", (res.stdout, res.stderr)


def test_import_keeps_gil_disabled():
    check_gil_off("import sys, sphn; print(sys._is_gil_enabled())")


def test_gil_stays_disabled_after_use():
    # numpy is imported lazily by the extension on first use, check this does not turn the GIL on
    # either.
    check_gil_off(
        "import sys, sphn, numpy as np\n"
        "pcm = np.zeros(4800, dtype=np.float32)\n"
        "sphn.resample(pcm, 24000, 48000)\n"
        "sphn.OpusStreamWriter(48000).append_pcm(pcm[:960])\n"
        "print(sys._is_gil_enabled())\n"
    )


def test_concurrent_independent_calls():
    with tempfile.TemporaryDirectory() as tmp:

        def work(idx):
            freq = 300 + 100 * idx
            sr = 24000
            mono = tone(freq, sr)
            stereo = np.stack([mono, tone(2 * freq, sr)])
            for it in range(5):
                prefix = os.path.join(tmp, f"{idx}_{it}")
                # wav: mono and stereo round trips.
                sphn.write_wav(prefix + "_mono.wav", mono, sr)
                data, out_sr = sphn.read(prefix + "_mono.wav")
                assert out_sr == sr and data.shape == (1, sr), (data.shape, out_sr)
                assert np.abs(data[0] - mono).max() < 1e-3
                sphn.write_wav(prefix + "_stereo.wav", stereo, sr)
                data, out_sr = sphn.read(prefix + "_stereo.wav")
                assert out_sr == sr and data.shape == (2, sr)
                assert np.abs(data - stereo).max() < 1e-3
                part, _ = sphn.read(prefix + "_stereo.wav", start_sec=0.25, duration_sec=0.5)
                assert abs(part.shape[-1] - sr // 2) <= 2
                data, out_sr = sphn.read(prefix + "_stereo.wav", sample_rate=16000)
                assert out_sr == 16000 and abs(peak_freq(data[0], 16000) - freq) < 5

                # file reader owned by this thread.
                fr = sphn.FileReader(prefix + "_stereo.wav")
                assert fr.sample_rate == sr and fr.channels == 2
                assert abs(fr.duration_sec - 1.0) < 0.01
                dec, _ = fr.decode_with_padding(0.5, 1.0)
                assert dec.shape == (2, sr)
                assert fr.decode_all().shape == (2, sr)

                # opus files and bytes.
                sphn.write_opus(prefix + ".opus", stereo, sr)
                data, out_sr = sphn.read_opus(prefix + ".opus")
                assert out_sr == 48000 and data.shape[0] == 2
                assert abs(peak_freq(data[0], 48000) - freq) < 5
                assert abs(peak_freq(data[1], 48000) - 2 * freq) < 5
                with open(prefix + ".opus", "rb") as f:
                    data_b, out_sr_b = sphn.read_opus_bytes(f.read())
                assert out_sr_b == 48000 and np.array_equal(data, data_b)

                # resample.
                res = sphn.resample(mono, sr, 48000)
                assert res.ndim == 1 and res.shape[0] >= 2 * sr - 64
                assert abs(peak_freq(res, 48000) - freq) < 5
                res = sphn.resample(stereo, sr, 16000)
                assert res.shape[0] == 2 and abs(peak_freq(res[1], 16000) - 2 * freq) < 5

                # durations, this uses rayon from inside the call.
                paths = [prefix + "_mono.wav", prefix + "_stereo.wav", prefix + "_missing.wav"]
                durs = sphn.durations(paths)
                assert durs[2] is None
                assert all(abs(d - 1.0) < 0.01 for d in durs[:2]), durs

        run_threads(work)


def _opus_chunks(n_frames=40, sr=48000):
    """Encodes some audio with a single writer, returns the list of ogg chunks."""
    writer = sphn.OpusStreamWriter(sr)
    pcm = tone(440, sr, dur=n_frames * 960 / sr)
    return [bytes(writer.append_pcm(pcm[i * 960 : (i + 1) * 960])) for i in range(n_frames)]


def test_shared_opus_stream_writer():
    # The writer is a mutable pyclass: concurrent calls must either work or fail with the pyo3
    # borrow checker's RuntimeError, nothing else.
    writer = sphn.OpusStreamWriter(48000)
    pcm = tone(440, 48000, dur=0.02)
    assert pcm.shape == (960,)
    n_calls = 100
    results, borrow_errors = [], []
    lock = threading.Lock()

    def work(idx):
        for _ in range(n_calls):
            try:
                out = bytes(writer.append_pcm(pcm))
            except RuntimeError as e:
                assert BORROW_ERROR.search(str(e)), e
                with lock:
                    borrow_errors.append(idx)
            else:
                with lock:
                    results.append(out)

    run_threads(work)
    assert len(results) + len(borrow_errors) == N_THREADS * n_calls
    assert len(results) > 0
    assert all(len(r) > 0 for r in results)
    # The header must have been sent exactly once.
    assert sum(b"OpusHead" in r for r in results) == 1


def test_shared_opus_stream_reader():
    chunks = _opus_chunks()
    reader = sphn.OpusStreamReader(48000)
    expected = np.concatenate([reader.append_bytes(c) for c in chunks])
    assert expected.shape[0] > 0

    # Sharing the reader and serializing the calls by hand gives the same result as using it from
    # a single thread, whichever thread does the call.
    reader = sphn.OpusStreamReader(48000)
    lock = threading.Lock()
    got = []
    todo = list(chunks)

    def locked(idx):
        while True:
            with lock:
                if not todo:
                    return
                got.append(reader.append_bytes(todo.pop(0)))

    run_threads(locked)
    assert np.array_equal(np.concatenate(got), expected)

    # Unsynchronized use: the decoded data is garbage as the chunks interleave, but this must be
    # a clean python exception (borrow error, or the decoder rejecting the stream), not a crash.
    reader = sphn.OpusStreamReader(48000)

    def hammer(idx):
        for _ in range(10):
            for c in chunks:
                try:
                    reader.append_bytes(c)
                except RuntimeError as e:
                    assert BORROW_ERROR.search(str(e)), e
                except ValueError:
                    pass

    run_threads(hammer)


def test_shared_file_reader():
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "shared.wav")
        sr = 16000
        sphn.write_wav(path, tone(440, sr, dur=3.0), sr)
        fr = sphn.FileReader(path)

        def work(idx):
            for i in range(50):
                try:
                    if idx % 2 == 0:
                        data = fr.decode((i % 4) * 0.5, 0.5)
                        assert data.shape[0] == 1 and abs(data.shape[1] - sr // 2) <= 2
                    else:
                        # Getters only need a shared borrow, they only fail while a decode runs.
                        assert fr.sample_rate == sr and fr.channels == 1
                        assert abs(fr.duration_sec - 3.0) < 0.01
                except RuntimeError as e:
                    assert BORROW_ERROR.search(str(e)), e

        run_threads(work)
        # The reader is still usable once the threads are done.
        assert fr.decode_all().shape == (1, 3 * sr)


def _make_dataset_files(tmp, n_files=6, sr=16000, dur=3.0):
    paths = []
    for i in range(n_files):
        path = os.path.join(tmp, f"f{i}.wav")
        sphn.write_wav(path, tone(200 + 50 * i, sr, dur=dur), sr)
        paths.append((path, dur))
    return paths


def _summary(sample):
    return (
        sample["sample_index"],
        sample["file_index"],
        sample["start_time_sec"],
        sample["data"].shape,
        float(np.abs(sample["data"]).sum()),
    )


def test_shared_dataset_reader():
    # A DatasetReader is immutable, each call to iter() creates a new DatasetIter with its own
    # worker threads so sharing it between threads must give each of them the full dataset.
    with tempfile.TemporaryDirectory() as tmp:
        paths = _make_dataset_files(tmp)
        calls = []
        lock = threading.Lock()

        def f(sample):
            # The callback runs on the dataset worker threads, concurrently on a free-threaded
            # build.
            with lock:
                calls.append(sample["sample_index"])
            return sample

        for callback in [None, f]:
            reader = sphn.DatasetReader(
                paths, duration_sec=0.5, num_threads=3, f=callback, on_error="raise"
            )
            for r in [reader, reader.shuffle(seed=42)]:
                expected = [_summary(s) for s in r]
                assert len(expected) == 5 * len(paths)
                outs = {}

                def work(idx):
                    outs[idx] = [_summary(s) for s in r]

                run_threads(work)
                assert len(outs) == N_THREADS
                assert all(o == expected for o in outs.values())
        assert len(calls) > 0


def test_shared_dataset_iter():
    # A DatasetIter is mutable (__next__ takes &mut self and releases the GIL while waiting for
    # the workers), concurrent calls to next must either work or raise a borrow error. Whatever
    # the interleaving, every sample is returned exactly once.
    with tempfile.TemporaryDirectory() as tmp:
        paths = _make_dataset_files(tmp)
        reader = sphn.DatasetReader(paths, duration_sec=0.5, num_threads=3, on_error="raise")
        n_expected = 5 * len(paths)
        for _ in range(3):
            it = iter(reader)
            seen, n_borrow = [], []
            lock = threading.Lock()

            def work(idx):
                while True:
                    try:
                        sample = next(it)
                    except StopIteration:
                        return
                    except RuntimeError as e:
                        assert BORROW_ERROR.search(str(e)), e
                        with lock:
                            n_borrow.append(idx)
                        continue
                    with lock:
                        seen.append(sample["sample_index"])

            run_threads(work)
            assert sorted(seen) == list(range(n_expected)), (sorted(seen), len(n_borrow))


def test_abandoned_dataset_iter_does_not_hang_exit():
    # The iterator workers are plain rust threads that attach to the interpreter to build samples
    # and call the user callback, leaving an iterator half consumed must not prevent exiting.
    with tempfile.TemporaryDirectory() as tmp:
        paths = _make_dataset_files(tmp)
        code = (
            "import sphn\n"
            f"paths = {paths!r}\n"
            "r = sphn.DatasetReader(paths, duration_sec=0.5, num_threads=3, f=lambda d: d)\n"
            "it = iter(r.shuffle(with_replacement=True))\n"
            "next(it)\n"
            "print('done')\n"
        )
        res = subprocess.run(
            [sys.executable, "-c", code], capture_output=True, text=True, timeout=120
        )
        assert res.returncode == 0, res.stderr
        assert res.stdout.strip() == "done", (res.stdout, res.stderr)


if __name__ == "__main__":
    n_failed = 0
    for name, fn in sorted(globals().items()):
        if not (name.startswith("test_") and callable(fn)):
            continue
        try:
            fn()
        except unittest.SkipTest as e:
            print(f"SKIP {name}: {e}")
        except BaseException as e:  # noqa: B902
            n_failed += 1
            print(f"FAIL {name}: {e!r}")
        else:
            print(f"ok   {name}")
    sys.exit(1 if n_failed else 0)
