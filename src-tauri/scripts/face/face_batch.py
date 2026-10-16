"""Face batch runner for xreenshoty: YuNet detect -> 5-point align -> SFace
embed, one JSON document on stdout. Mirrors the spike; cv2 owns detection,
raw onnxruntime owns recognition."""
import json
import os
import sys
import time

import numpy as np

# SFace's ONNX is full of `Initializer X appears in graph inputs` graph
# warnings (~80 KB per session!). On a piped stderr that fills the 64 KB pipe
# buffer before the parent reads and deadlocks the whole batch. Severity is
# numeric on purpose: ORT accepts 0..4 only, and `logging.ERROR` (40) raises
# "Invalid logging severity" — which a bare `except: pass` would swallow.
try:
    import onnxruntime

    onnxruntime.set_default_logger_severity(3)  # 0=V 1=I 2=W 3=E 4=Fatal
except Exception:
    pass

MODELS = os.environ.get(
    "XREENSHOTY_FACE_MODELS",
    os.path.expanduser("~/apps/models"),
)
YUNET = os.path.join(MODELS, "face_detection_yunet_2023mar.onnx")
SFACE = os.path.join(MODELS, "face_recognition_sface_2021dec.onnx")
MIN_SIDE = float(os.environ.get("XREENSHOTY_FACE_MIN_PX", "80"))
SCORE = float(os.environ.get("XREENSHOTY_FACE_SCORE", "0.6"))

TEMPLATE = np.array(
    [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ],
    dtype=np.float64,
)

_rec = None
_det = None


def _sessions():
    global _rec, _det
    if _rec is None:
        import onnxruntime as ort

        _rec = ort.InferenceSession(SFACE, providers=["CPUExecutionProvider"])
    if _det is None:
        import cv2

        _det = cv2.FaceDetectorYN.create(YUNET, "", (640, 640), SCORE, 0.3, 5000)
    return _det, _rec


def _load(path):
    from PIL import Image

    with Image.open(path) as im:
        return np.asarray(im.convert("RGB"))


def _align(img, lms):
    from PIL import Image

    A, _, _, _ = np.linalg.lstsq(
        np.hstack([lms, np.ones((5, 1))]), TEMPLATE, rcond=None
    )
    M = np.linalg.inv(np.vstack([A.T, [0, 0, 1]]))[:2]
    return np.asarray(
        Image.fromarray(img).transform(
            (112, 112), Image.AFFINE, M.flatten().tolist(),
            resample=Image.BILINEAR,
        )
    )


def faces_of(path):
    import cv2

    det, rec = _sessions()
    img = _load(path)
    h, w = img.shape[:2]
    sw, sh = max(32, (w // 32) * 32), max(32, (h // 32) * 32)
    det.setInputSize((sw, sh))
    small = cv2.resize(cv2.cvtColor(img, cv2.COLOR_RGB2BGR), (sw, sh))
    _, raw = det.detect(small)
    out = []
    if raw is None:
        return out
    rin = rec.get_inputs()[0].name
    rout = rec.get_outputs()[0].name
    fx, fy = w / sw, h / sh
    for r in raw:
        bw, bh = float(r[2] * fx), float(r[3] * fy)
        if min(bw, bh) < MIN_SIDE:
            continue
        lms = r[4:14].astype(np.float64).reshape(5, 2)
        lms[:, 0] *= fx
        lms[:, 1] *= fy
        crop = _align(img, lms)
        x = ((crop.astype(np.float32) - 127.5) / 128.0).transpose(2, 0, 1)[None]
        emb = rec.run([rout], {rin: x})[0][0]
        out.append(
            {
                "box": [round(float(r[0] * fx), 1), round(float(r[1] * fy), 1),
                        round(bw, 1), round(bh, 1)],
                "score": round(float(r[-1]), 3),
                "embedding": [round(float(v), 6) for v in emb],
            }
        )
    return out


def main():
    t0 = time.time()
    try:
        paths = json.load(sys.stdin)
    except Exception as e:
        print(json.dumps({"ok": False, "error": f"bad stdin json: {e}"}))
        return
    if not isinstance(paths, list):
        print(json.dumps({"ok": False, "error": "stdin must be a JSON list"}))
        return
    for f in (YUNET, SFACE):
        if not os.path.isfile(f):
            print(json.dumps({"ok": False, "error": f"missing model: {f}"}))
            return
    results, errors = [], []
    try:
        for p in paths:
            try:
                results.append({"path": p, "faces": faces_of(p)})
            except Exception as e:
                errors.append({"path": p, "error": str(e)[:300]})
    except ImportError as e:
        print(json.dumps({"ok": False,
                          "error": f"face-env incomplete (pip install opencv-python-headless onnxruntime numpy pillow): {e}"}))
        return
    print(json.dumps({"ok": True, "results": results, "errors": errors,
                      "ms": round((time.time() - t0) * 1000, 1)}))


main()
