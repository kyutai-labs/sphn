# Generated content DO NOT EDIT
from typing import Any, Callable, Dict, List, Optional, Tuple, Union, Sequence
from os import PathLike

@staticmethod
def durations(filenames):
    """
    Returns the durations for the audio files passed as input.

    The input argument is a list of filenames. For each of these files, the duration in seconds is
    returned as a float, None is returned if the files cannot be open or properly read.
    """
    pass

@staticmethod
def read(filename, start_sec=None, duration_sec=None, sample_rate=None):
    """
    Reads the content of an audio file and returns it as a numpy array.

    The input argument is a filename. Its content is decoded the audio data for the whole file and
    return it as a two dimensional numpy array as well as the sample rate.
    """
    pass

@staticmethod
def read_opus(filename):
    """
    Reads the whole content of an ogg/opus encoded file.

    This returns a two dimensional array as well as the sample rate. Currently all opus audio is
    encoded at 48kHz so this value is always returned.
    """
    pass

@staticmethod
def read_opus_bytes(bytes):
    """
    Reads bytes corresponding to an ogg/opus encoded file.

    This returns a two dimensional array as well as the sample rate. Currently all opus audio is
    encoded at 48kHz so this value is always returned.
    """
    pass

@staticmethod
def resample(pcm, src_sample_rate, dst_sample_rate):
    """
    Resamples some pcm data.
    """
    pass

@staticmethod
def write_opus(filename, data, sample_rate):
    """
    Writes an opus file containing the input pcm data.

    Opus content is always encoded at 48kHz so the pcm data is resampled if sample_rate is
    different from 48000.
    """
    pass

@staticmethod
def write_wav(filename, data, sample_rate):
    """
    Writes an audio file using the wav format based on pcm data from a numpy array.

    This only supports a single channel at the moment so the input array data is expected to have a
    single dimension.
    """
    pass

class FileReader:
    def __init__(path):
        pass

    @property
    def channels(self):
        """
        The number of channels.
        """
        pass

    def decode(self, start_sec, duration_sec):
        """
        Decodes the audio data from `start_sec` to `start_sec + duration_sec` and return the PCM
        data as a two dimensional numpy array. The first dimension is the channel, the second one
        is time.
        If the end of the file is reached, the decoding stops and the already decoded data is
        returned.
        """
        pass

    def decode_all(self):
        """
        Decodes the audio data for the whole file and return it as a two dimensional numpy array.
        """
        pass

    def decode_with_padding(self, start_sec, duration_sec):
        """
        Decodes the audio data from `start_sec` to `start_sec + duration_sec` and return the PCM
        data as a two dimensional numpy array. The first dimension is the channel, the second one
        is time.
        If the end of the file is reached, the array is padded with zeros so that its length is
        still matching `duration_sec`.
        """
        pass

    @property
    def duration_sec(self):
        """
        The duration of the audio stream in seconds.
        """
        pass

    @property
    def sample_rate(self):
        """
        The sample rate as an int.
        """
        pass

class OpusStreamReader:
    def __init__(sample_rate):
        pass

    def append_bytes(self, data):
        """
        Write some ogg/opus bytes to the current stream.
        """
        pass

    def read_pcm(self):
        """
        Get some pcm data out of the stream.
        """
        pass

class OpusStreamWriter:
    def __init__(sample_rate):
        pass

    def append_pcm(self, pcm):
        """ """
        pass

    def read_bytes(self):
        """ """
        pass
