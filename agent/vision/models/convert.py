"""Converts MediaPipe's face detectors to the ONNX files tessaro-vision runs.

Run through `mise run vision:models`, in a throwaway python container: tf2onnx
is a python tool and nothing else in the repo needs it. The .tflite files are
fetched from MediaPipe's v0.8.9 tag and checked against their sha256 first, so
a converted model is always the same weights. Writes face-full.onnx and
face-short.onnx next to this file.
"""

import hashlib
import pathlib
import subprocess
import sys
import urllib.request

BASE = "https://raw.githubusercontent.com/google/mediapipe/v0.8.9/mediapipe/modules/face_detection/"

# name -> (tflite, sha256). face_detection_full_range is the file Human calls
# blazeface-back, face_detection_short_range the one it calls blazeface-front.
MODELS = {
    "face-full": (
        "face_detection_full_range.tflite",
        "99bf9494d84f50acc6617d89873f71bf6635a841ea699c17cb3377f9507cfec3",
    ),
    "face-short": (
        "face_detection_short_range.tflite",
        "3bc182eb9f33925d9e58b5c8d59308a760f4adea8f282370e428c51212c26633",
    ),
}

here = pathlib.Path(__file__).resolve().parent
work = pathlib.Path("/tmp/models")
work.mkdir(exist_ok=True)

for name, (tflite, sha) in MODELS.items():
    source = work / tflite
    with urllib.request.urlopen(BASE + tflite) as response:
        source.write_bytes(response.read())
    digest = hashlib.sha256(source.read_bytes()).hexdigest()
    if digest != sha:
        sys.exit(f"{tflite}: sha256 {digest}, expected {sha}")
    subprocess.run(
        [sys.executable, "-m", "tf2onnx.convert", "--tflite", str(source),
         "--output", str(here / f"{name}.onnx"), "--opset", "13"],
        check=True,
    )
    print(f"{name}.onnx from {tflite}")
