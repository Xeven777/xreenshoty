use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;

/// One recognised word with its box, in image pixel coordinates.
#[derive(Debug, Clone, Serialize)]
pub struct Word {
    pub text: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub confidence: f32,
}

#[derive(Debug, Default)]
pub struct OcrResult {
    pub text: String,
    pub words: Vec<Word>,
}

/// How long a single tesseract run may take before it is killed.
///
/// Deliberately generous. A screenshot is ~0.5s normally, so this is not a
/// performance knob — it exists only to stop a wedged subprocess from hanging
/// the indexer for the rest of the session. It cannot be tightened without
/// risk: a failed row is marked attempted (see the indexer) and so is never
/// retried, which means a false timeout silently loses that screenshot's text.
fn timeout() -> Duration {
    let secs = std::env::var("XREENSHOTY_OCR_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(60);
    Duration::from_secs(secs)
}

/// Runs `cmd` to completion, killing it if it overruns `limit`.
///
/// `Command::output()` waits forever, and a wedged tesseract would hold the
/// indexer's `thread::scope` join open, stopping indexing entirely — with no
/// log line and nothing for the user to see. Polling `try_wait` needs no new
/// dependency and this already runs on a dedicated worker thread.
fn run_with_timeout(cmd: &mut Command, limit: Duration) -> Result<std::process::Output, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("tesseract not runnable: {e}"))?;

    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => return Err(format!("waiting on tesseract failed: {e}")),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "tesseract exceeded {}s and was killed",
                limit.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    child
        .wait_with_output()
        .map_err(|e| format!("tesseract output unreadable: {e}"))
}

/// Runs tesseract and parses its TSV output.
///
/// TSV carries a level column: rows below level 5 are layout scaffolding
/// (page, block, paragraph, line) with no text, so only level 5 rows are actual
/// words. Boxes come free from the same output, which is why TSV is preferred
/// over plain text output here.
pub fn run(image: &Path) -> Result<OcrResult, String> {
    let mut cmd = Command::new("tesseract");
    cmd.arg(image)
        .arg("stdout")
        .arg("tsv")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = run_with_timeout(&mut cmd, timeout())?;

    if !output.status.success() {
        return Err(format!(
            "tesseract failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_tsv(&String::from_utf8_lossy(&output.stdout)))
}

pub fn parse_tsv(tsv: &str) -> OcrResult {
    let mut words = Vec::new();
    let mut lines = Vec::new();

    for line in tsv.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        let level = cols[0].trim();
        let text = cols[11].trim();
        if level != "5" || text.is_empty() {
            continue;
        }
        let Ok(confidence) = cols[10].trim().parse::<f32>() else {
            continue;
        };
        // Layout-level rows carry -1; genuine words carry a real score.
        if confidence < 0.0 {
            continue;
        }
        let nums = [cols[6], cols[7], cols[8], cols[9]].map(|c| c.trim().parse::<u32>().unwrap_or(0));
        words.push(Word {
            text: text.to_string(),
            x: nums[0],
            y: nums[1],
            w: nums[2],
            h: nums[3],
            confidence,
        });
        lines.push(text);
    }

    OcrResult {
        text: lines.join(" "),
        words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
1\t1\t0\t0\t0\t0\t0\t0\t100\t50\t-1\t\n\
2\t1\t1\t0\t0\t0\t0\t0\t100\t50\t-1\t\n\
5\t1\t1\t1\t1\t1\t11\t17\t19\t12\t88.807754\t#4\n\
5\t1\t1\t1\t1\t2\t62\t14\t28\t9\t96.967636\tone\n\
5\t1\t1\t1\t2\t1\t10\t2\t8\t5\t-1\t\n";

    #[test]
    fn keeps_only_word_rows_and_drops_empty_confidence() {
        let result = parse_tsv(SAMPLE);
        assert_eq!(result.text, "#4 one");
        assert_eq!(result.words.len(), 2);

        let first = &result.words[0];
        assert_eq!((first.x, first.y, first.w, first.h), (11, 17, 19, 12));
        assert!((first.confidence - 88.807754).abs() < 0.001);
    }

    #[test]
    fn empty_input_is_not_an_error() {
        let result = parse_tsv("level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n");
        assert!(result.text.is_empty());
        assert!(result.words.is_empty());
    }

    /// BM9: the whole point is that this returns instead of hanging.
    #[test]
    fn a_wedged_process_is_killed_rather_than_waited_on() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let started = std::time::Instant::now();
        let err = run_with_timeout(&mut cmd, Duration::from_millis(100)).unwrap_err();
        let took = started.elapsed();

        assert!(err.contains("killed"), "got {err}");
        assert!(
            took < Duration::from_secs(5),
            "returned in {took:?} — it waited on the process instead of killing it"
        );
    }

    #[test]
    fn a_process_that_finishes_is_left_alone() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let out = run_with_timeout(&mut cmd, Duration::from_secs(30)).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }
}