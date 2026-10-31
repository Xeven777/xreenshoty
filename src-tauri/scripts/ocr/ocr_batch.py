"""OCR batch runner for xreenshoty: Indra OCR over a list of screenshot paths.

One JSON document on stdout, same shape as face_batch.py: the Rust side sends
the paths on stdin and reads the answer back.

    {"ok": true,
     "results": [{"path": "...", "text": "...",
                  "lines": [{"box": [x0, y0, x1, y1], "text": "...", "conf": 0.93}]}],
     "errors": [{"path": "...", "error": "..."}]}

Indra gives boxes per *line*, not per word. The Rust side splits a line into
words itself; the boxes it stores are therefore a proportional guess and are
not pixel-accurate.
"""
import json
import os
import sys
import time

# ONNX Runtime's graph warnings fill stderr, and a Linux pipe holds only 64 KB:
# a parent that reads the pipes only after the child exits deadlocks the batch.
# Severity is numeric on purpose — ORT accepts 0..4 only, and `logging.ERROR`
# (40) raises "Invalid logging severity", which a bare `except: pass` hides.
try:
    import onnxruntime

    onnxruntime.set_default_logger_severity(3)  # 0=V 1=I 2=W 3=E 4=Fatal
except Exception:
    pass

MODEL_DIR = os.environ.get(
    "XREENSHOTY_OCR_MODELS",
    os.path.expanduser("~/apps/models/indra-ocr"),
)
# Worker processes, one page each. 0 means "all cores this process may run on".
CORES = int(os.environ.get("XREENSHOTY_OCR_CORES", "0"))


def lines_of(result):
    """Every recognised line of a file, in reading order.

    One page for an image; a multi-frame TIFF gives one page per frame, and
    they are concatenated rather than the last one winning. A page whose
    source is "error" is a failure, not an empty page -- recording it as "no
    text found" would mark a perfectly good screenshot as text-free forever.
    """
    lines = []
    for _index, page in result:
        if page.source == "error":
            return None, page.note
        lines.extend(
            {"box": ln.box, "text": ln.text, "conf": ln.conf} for ln in page.lines
        )
    return lines, ""


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

    try:
        import indra_ocr
    except ImportError as e:
        print(json.dumps({
            "ok": False,
            "error": f"ocr-env incomplete (pip install indra-ocr): {e}",
        }))
        return

    results, errors = [], []
    try:
        kwargs = {"path": MODEL_DIR}
        if CORES:
            kwargs["cores"] = CORES
        for path, result in indra_ocr.convert_many(paths, **kwargs):
            if isinstance(result, indra_ocr.InputError):
                errors.append({"path": path, "error": str(result)[:300]})
                continue
            lines, note = lines_of(result)
            if lines is None:
                errors.append({"path": path, "error": (note or "read failed")[:300]})
                continue
            results.append({
                "path": path,
                "text": "\n".join(ln["text"] for ln in lines),
                "lines": lines,
            })
    except Exception as e:
        # A missing model folder raises ModelError here, before any page is read.
        print(json.dumps({
            "ok": False,
            "error": f"{type(e).__name__}: {str(e)[:300]}",
        }))
        return

    print(json.dumps({
        "ok": True,
        "results": results,
        "errors": errors,
        "ms": round((time.time() - t0) * 1000, 1),
    }, ensure_ascii=False))


if __name__ == "__main__":
    main()
