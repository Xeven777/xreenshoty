use std::path::Path;
use std::process::Command;

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

/// Runs tesseract and parses its TSV output.
///
/// TSV carries a level column: rows below level 5 are layout scaffolding
/// (page, block, paragraph, line) with no text, so only level 5 rows are actual
/// words. Boxes come free from the same output, which is why TSV is preferred
/// over plain text output here.
pub fn run(image: &Path) -> Result<OcrResult, String> {
    let output = Command::new("tesseract")
        .arg(image)
        .arg("stdout")
        .arg("tsv")
        .output()
        .map_err(|e| format!("tesseract not runnable: {e}"))?;

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
}