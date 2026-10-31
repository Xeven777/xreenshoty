#!/usr/bin/env bash
# One-shot install of the face runtime: the `face-env` venv and the two ONNX
# models. The app never downloads anything itself — it runs offline.
#
#   ./scripts/setup-face.sh
#
# Overridable: XREENSHOTY_FACE_MODELS (model dir). There is deliberately no venv
# override: `face.rs` looks only at <data-local>/xreenshoty/face-env/bin/python,
# so a relocated venv would be installed and then never found.
set -euo pipefail

ENV_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/xreenshoty/face-env"
MODELS_DIR="${XREENSHOTY_FACE_MODELS:-$HOME/apps/models}"
PY_VERSION="${XREENSHOTY_FACE_PYTHON_VERSION:-3.13}"

YUNET="face_detection_yunet_2023mar.onnx"
SFACE="face_recognition_sface_2021dec.onnx"
# The media host, not raw: opencv_zoo keeps the .onnx files in Git LFS, and
# raw.githubusercontent.com serves the 131-byte LFS *pointer* with a 200 status.
REPO="https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models"

if ! command -v uv >/dev/null 2>&1; then
  echo "uv is required: https://docs.astral.sh/uv/getting-started/installation/" >&2
  exit 1
fi

echo "==> venv at $ENV_DIR"
if [ ! -x "$ENV_DIR/bin/python" ]; then
  uv venv "$ENV_DIR" --python "$PY_VERSION"
else
  echo "   already there, keeping it"
fi
# YuNet detection runs through cv2.FaceDetectorYN, which owns the anchor math;
# SFace recognition runs on raw onnxruntime. No torch, no torchvision.
uv pip install --python "$ENV_DIR/bin/python" \
  onnxruntime opencv-python-headless numpy pillow

echo "==> models at $MODELS_DIR"
mkdir -p "$MODELS_DIR"

# Idempotent: a model is only fetched when it is missing or truncated. 100 kB is
# a floor, not a checksum — it catches the failure this script can actually
# cause, which is an HTML error page or a half-written file saved as `.onnx`.
fetch() {
  local name="$1" url="$2"
  local target="$MODELS_DIR/$name"
  if [ -s "$target" ] && [ "$(stat -c%s "$target")" -gt 100000 ]; then
    echo "   $name already there, keeping it"
    return
  fi
  echo "   downloading $name"
  "$ENV_DIR/bin/python" - "$url" "$target" "$name" <<'PY'
import os
import sys
import urllib.request

url, target, name = sys.argv[1], sys.argv[2], sys.argv[3]
tmp = target + ".part"
try:
    urllib.request.urlretrieve(url, tmp)
    size = os.path.getsize(tmp)
    with open(tmp, "rb") as fh:
        head = fh.read(64)
    if b"git-lfs.github.com" in head:
        sys.exit(
            f"{name}: got a Git LFS pointer ({size} bytes), not the model — "
            f"fetch from media.githubusercontent.com instead"
        )
    if size < 100000:
        sys.exit(f"{name}: got {size} bytes from {url} — not an ONNX model")
    os.replace(tmp, target)
    print(f"   {size:,} bytes")
finally:
    # A rejected download must not leave a .part behind: the next run skips only
    # files that pass the size floor, but a stray .part is just litter.
    if os.path.exists(tmp):
        os.remove(tmp)
PY
}

fetch "$YUNET" "$REPO/face_detection_yunet/$YUNET"
fetch "$SFACE" "$REPO/face_recognition_sface/$SFACE"

echo
echo "==> verifying"
"$ENV_DIR/bin/python" - "$MODELS_DIR" <<'PY'
import os
import sys

import cv2
import numpy
import onnxruntime
import PIL

# Same numeric scale as face_batch.py: ORT takes 0..4, not logging levels, and
# SFace's graph prints ~80 KB of "Initializer ... in graph inputs" warnings.
onnxruntime.set_default_logger_severity(3)

target = sys.argv[1]
yunet = os.path.join(target, "face_detection_yunet_2023mar.onnx")
sface = os.path.join(target, "face_recognition_sface_2021dec.onnx")

for path in (yunet, sface):
    size = os.path.getsize(path)
    if size < 100000:
        sys.exit(f"{path} looks truncated ({size} bytes)")

# Creating both really loads the graphs — a renamed HTML page fails here.
cv2.FaceDetectorYN.create(yunet, "", (640, 640), 0.6, 0.3, 5000)
session = onnxruntime.InferenceSession(sface, providers=["CPUExecutionProvider"])

print("   cv2", cv2.__version__, "| onnxruntime", onnxruntime.__version__)
print("   numpy", numpy.__version__, "| pillow", PIL.__version__)
print("   yunet", f"{os.path.getsize(yunet):,} bytes")
print("   sface", f"{os.path.getsize(sface):,} bytes", session.get_inputs()[0].shape)
PY

cat <<EOF

Done. The app picks this up on its next start.

  venv   $ENV_DIR
  models $MODELS_DIR

Face indexing stays OFF until you enable it in Settings. If you move the models,
point the app at them with XREENSHOTY_FACE_MODELS (the venv path is fixed).
EOF
