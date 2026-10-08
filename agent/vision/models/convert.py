"""Converts the models tessaro-vision runs to ONNX files.

Run through `mise run vision:models`, in a throwaway python container: tf2onnx
is a python tool and nothing else in the repo needs it. Every file is fetched
from a pinned tag or commit and checked against its sha256 first, so a
converted model is always the same weights. Writes the .onnx files next to this
file:

* face-full.onnx and face-short.onnx, MediaPipe's face detectors, from the
  .tflite files of its v0.8.9 tag;
* faceres.onnx, HSE FaceRes, the age and gender estimator, from the TFJS
  graph model `@vladmandic/human` runs by default (its human-models
  repository, which converted it from HSE-asavchenko/HSE_FaceRec_tf).
"""

import hashlib
import json
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

HUMAN = "https://raw.githubusercontent.com/vladmandic/human-models/bc66dc53bac03c96d35a7e6daaf717e72f3985f5/models/"

# name -> (graph, {file: sha256}). The graph's weights are the .bin next to
# it, which tf2onnx reads from the same directory.
GRAPHS = {
    "faceres": (
        "faceres.json",
        {
            "faceres.json": "5b83d49c0385d2e68a05122441b94226313677cae9fcc40b9587ad50079eb4df",
            "faceres.bin": "2c7d2d62b76c97528b736527aa09d310ea71743c9e3e79fb6c62d4b2d73af79b",
        },
    ),
}

here = pathlib.Path(__file__).resolve().parent
work = pathlib.Path("/tmp/models")
work.mkdir(exist_ok=True)


def fetch(url, sha):
    """The file at `url` in the work directory, refused unless it is `sha`."""
    target = work / url.rsplit("/", 1)[1]
    with urllib.request.urlopen(url) as response:
        target.write_bytes(response.read())
    digest = hashlib.sha256(target.read_bytes()).hexdigest()
    if digest != sha:
        sys.exit(f"{target.name}: sha256 {digest}, expected {sha}")
    return target


def with_targs(graph):
    """The TFJS graph with `TArgs` on every `_FusedConv2D`: the TFJS converter
    left it out, and the TensorFlow tf2onnx imports the graph with refuses a
    node without it. It is the type of each extra input, `T` for all of them,
    so the weights are the same."""
    model = json.loads(graph.read_text())
    for node in model["modelTopology"]["node"]:
        attr = node.get("attr", {})
        if node["op"] == "_FusedConv2D" and "TArgs" not in attr:
            args = int(attr["num_args"]["i"])
            attr["TArgs"] = {"list": {"type": [attr["T"]["type"]] * args}}
    graph.write_text(json.dumps(model))
    return graph


def convert(kind, source, name):
    subprocess.run(
        [sys.executable, "-m", "tf2onnx.convert", kind, str(source),
         "--output", str(here / f"{name}.onnx"), "--opset", "13"],
        check=True,
    )
    print(f"{name}.onnx from {source.name}")


for name, (tflite, sha) in MODELS.items():
    convert("--tflite", fetch(BASE + tflite, sha), name)

for name, (graph, files) in GRAPHS.items():
    for file, sha in files.items():
        fetch(HUMAN + file, sha)
    convert("--tfjs", with_targs(work / graph), name)
