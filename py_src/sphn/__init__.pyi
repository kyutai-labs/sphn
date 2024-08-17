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
