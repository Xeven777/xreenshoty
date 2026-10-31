#!/usr/bin/env bash
# One-shot install of the OCR runtime: the `ocr-env` venv and the Indra OCR
# model files. The app never downloads anything itself — it runs offline.
#
#   ./scripts/setup-ocr.sh
#
# Overridable: XREENSHOTY_OCR_ENV (venv dir), XREENSHOTY_OCR_MODELS (model dir).
set -euo pipefail

ENV_DIR="${XREENSHOTY_OCR_ENV:-$HOME/.local/share/xreenshoty/ocr-env}"
MODELS_DIR="${XREENSHOTY_OCR_MODELS:-$HOME/apps/models/indra-ocr}"
PY_VERSION="${XREENSHOTY_OCR_PYTHON_VERSION:-3.13}"

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
uv pip install --python "$ENV_DIR/bin/python" indra-ocr

echo "==> models at $MODELS_DIR"
mkdir -p "$MODELS_DIR"
# The same file set and revision indra_ocr pins: the .onnx models, the
# vocabulary and the language models used for unsure lines. Math is off, so the
# math models (21 MB) are left out — they are only loaded with math=True.
"$ENV_DIR/bin/python" - "$MODELS_DIR" <<'PY'
import sys
from huggingface_hub import snapshot_download

target = sys.argv[1]
path = snapshot_download(
    "datalab-to/indra-ocr",
    revision="v1.1.0",
    local_dir=target,
    allow_patterns=["*.onnx", "meta.json", "lm.npz", "lm_*.npz"],
    ignore_patterns=[
        "mathcls.onnx",
        "math_enc.onnx",
        "math_dec_init.onnx",
        "math_dec_step.onnx",
        "math_meta.json",
    ],
)
print("downloaded to", path)
PY

echo
echo "==> verifying"
INDRA_OCR_MODEL_DIR="$MODELS_DIR" "$ENV_DIR/bin/python" - <<'PY'
import indra_ocr

print("indra_ocr", indra_ocr.__version__)
print("models at", indra_ocr.model_dir())
PY

cat <<EOF

Done. The app picks this up on its next start.

  venv   $ENV_DIR
  models $MODELS_DIR

If you move either, point the app at it with
XREENSHOTY_OCR_PYTHON and XREENSHOTY_OCR_MODELS.
EOF
