//! `expect.logs`: assertions on what the loadsmith container printed — its
//! stdout, its stderr and the JSON run summary it wrote — so a case can prove a
//! secret never leaks (`not_contains`) or that a message was emitted
//! (`contains`).
//!
//! `not_contains` needles are secrets: the lab never prints one in full. A
//! failure names a short prefix and the length, and [`Masker`] scrubs the
//! needles from the live gutter and from every failure line.

use std::borrow::Cow;

use crate::case::LogsExpect;

/// The loadsmith container's captured output.
#[derive(Debug, Clone, Default)]
pub struct Captured {
    pub stdout: String,
    pub stderr: String,
    /// The JSON run summary's raw text; `None` on the text-fallback path
    /// (engine without `--summary-file`) or when the file was not written.
    pub summary: Option<String>,
}

impl Captured {
    /// Each stream by name, ANSI-stripped, in report order.
    fn streams(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![("stdout", strip_ansi(&self.stdout)), ("stderr", strip_ansi(&self.stderr))];
        if let Some(s) = &self.summary {
            v.push(("summary file", strip_ansi(s)));
        }
        v
    }
}

/// Checks `expect` against `captured`; one failure line per unmet needle.
pub fn check_logs(expect: &LogsExpect, captured: &Captured) -> Vec<String> {
    let streams = captured.streams();
    let mut failures = Vec::new();

    for needle in &expect.not_contains {
        if needle.is_empty() {
            failures.push("logs.not_contains: empty needle (it would match anything)".to_string());
            continue;
        }
        let leaked: Vec<&str> = streams
            .iter()
            .filter(|(_, text)| text.contains(needle.as_str()))
            .map(|(name, _)| *name)
            .collect();
        if !leaked.is_empty() {
            failures.push(format!(
                "logs.not_contains: needle {} found in {}",
                describe_secret(needle),
                leaked.join(", ")
            ));
        }
    }

    for needle in &expect.contains {
        if needle.is_empty() {
            failures.push("logs.contains: empty needle (it would match anything)".to_string());
            continue;
        }
        if !streams.iter().any(|(_, text)| text.contains(needle.as_str())) {
            let names: Vec<&str> = streams.iter().map(|(n, _)| *n).collect();
            failures.push(format!("logs.contains: {needle:?} not found in {}", names.join(", ")));
        }
    }

    failures
}

/// A secret named without revealing it: a prefix of at most a quarter of its
/// characters (capped at 4), then its length — `"s3cr…" (16 chars)`.
pub fn describe_secret(needle: &str) -> String {
    let n = needle.chars().count();
    let prefix: String = needle.chars().take((n / 4).min(4)).collect();
    format!("\"{prefix}…\" ({n} chars)")
}

/// Scrubs `not_contains` needles out of text the lab prints (gutter lines,
/// failure lines), replacing each with `[redacted "s3cr…" (16 chars)]`.
#[derive(Debug, Clone, Default)]
pub struct Masker {
    needles: Vec<String>,
}

impl Masker {
    pub fn new(expect: Option<&LogsExpect>) -> Self {
        let mut needles: Vec<String> = expect
            .map(|e| e.not_contains.iter().filter(|n| !n.is_empty()).cloned().collect())
            .unwrap_or_default();
        // Longest first, so a needle that contains another is masked whole.
        needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
        Masker { needles }
    }

    /// `text` unchanged when it holds no needle; else ANSI-stripped (a needle
    /// may be split by colour codes) with every needle replaced.
    pub fn mask<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.needles.is_empty() {
            return Cow::Borrowed(text);
        }
        let plain = strip_ansi(text);
        if !self.needles.iter().any(|n| plain.contains(n.as_str())) {
            return Cow::Borrowed(text);
        }
        let mut out = plain;
        for n in &self.needles {
            out = out.replace(n.as_str(), &format!("[redacted {}]", describe_secret(n)));
        }
        Cow::Owned(out)
    }
}

/// Removes ANSI escape sequences: CSI (`ESC [ … final`), OSC (`ESC ] … BEL` or
/// `ESC ] … ESC \`), `ESC <intermediates> final` and two-byte `ESC x` forms.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                // Parameter/intermediate bytes, then one final byte @..~.
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{07}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // `ESC <intermediates 0x20-0x2F> <final>` (e.g. `ESC ( B`): drop
            // the intermediates and the final byte.
            Some(c) if (' '..='/').contains(&c) => {
                for c in chars.by_ref() {
                    if !(' '..='/').contains(&c) {
                        break;
                    }
                }
            }
            // Two-byte escape (or a lone trailing ESC): drop both.
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "s3cr3t-value-0123";

    fn expect(contains: &[&str], not_contains: &[&str]) -> LogsExpect {
        LogsExpect {
            contains: contains.iter().map(|s| s.to_string()).collect(),
            not_contains: not_contains.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn captured(stdout: &str, stderr: &str, summary: Option<&str>) -> Captured {
        Captured { stdout: stdout.into(), stderr: stderr.into(), summary: summary.map(Into::into) }
    }

    #[test]
    fn strips_ansi_sequences() {
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m done"), "ok done");
        assert_eq!(strip_ansi("a\u{1b}]8;;http://x\u{07}link\u{1b}]8;;\u{1b}\\b"), "alinkb");
        assert_eq!(strip_ansi("x\u{1b}(By"), "xy");
        assert_eq!(strip_ansi("plain │ text"), "plain │ text");
    }

    #[test]
    fn clean_output_passes() {
        let c = captured("INFO schema negotiated\n", "", Some(r#"{"status":"success"}"#));
        assert!(check_logs(&expect(&["schema negotiated"], &[SECRET, "BEGIN PRIVATE KEY"]), &c).is_empty());
    }

    #[test]
    fn leak_names_every_stream_it_reached() {
        let c = captured("", &format!("error: auth failed for {SECRET}"), Some(&format!(r#"{{"cause":"{SECRET}"}}"#)));
        let f = check_logs(&expect(&[], &[SECRET]), &c);
        assert_eq!(f, vec![r#"logs.not_contains: needle "s3cr…" (17 chars) found in stderr, summary file"#.to_string()]);
    }

    #[test]
    fn leak_failure_never_prints_the_needle() {
        let c = captured(SECRET, SECRET, Some(SECRET));
        for line in check_logs(&expect(&[], &[SECRET]), &c) {
            assert!(!line.contains(SECRET), "{line}");
            assert!(!line.contains("s3cr3t"), "more than a short prefix: {line}");
        }
        // Short needles reveal proportionally less — a 3-char one reveals nothing.
        assert_eq!(describe_secret("abc"), "\"…\" (3 chars)");
        assert_eq!(describe_secret("pass1234"), "\"pa…\" (8 chars)");
        assert_eq!(describe_secret("ção-segredo"), "\"çã…\" (11 chars)");  // char-, not byte-, based
    }

    #[test]
    fn ansi_cannot_hide_a_leak_or_a_match() {
        let coloured = "token=s3cr\u{1b}[0m3t-value-0123 \u{1b}[32mschema\u{1b}[0m negotiated";
        let f = check_logs(&expect(&["schema negotiated"], &[SECRET]), &captured(coloured, "", None));
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("found in stdout"), "{f:?}");
    }

    #[test]
    fn missing_positive_needle_fails() {
        let f = check_logs(&expect(&["schema negotiated"], &[]), &captured("x", "y", None));
        assert_eq!(f, vec![r#"logs.contains: "schema negotiated" not found in stdout, stderr"#.to_string()]);
        let f = check_logs(&expect(&["nope"], &[]), &captured("x", "y", Some("{}")));
        assert!(f[0].ends_with("stdout, stderr, summary file"), "{f:?}");
    }

    #[test]
    fn positive_needle_may_be_in_any_stream() {
        for c in [captured("hit", "", None), captured("", "hit", None), captured("", "", Some("hit"))] {
            assert!(check_logs(&expect(&["hit"], &[]), &c).is_empty(), "{c:?}");
        }
    }

    #[test]
    fn empty_needles_are_rejected() {
        let f = check_logs(&expect(&[""], &[""]), &captured("x", "", None));
        assert_eq!(f.len(), 2, "{f:?}");
    }

    #[test]
    fn masker_scrubs_needles() {
        let e = expect(&[], &[SECRET, "s3cr3t"]);
        let m = Masker::new(Some(&e));
        assert_eq!(m.mask("no secret here"), "no secret here");
        let line = format!("cause: login {SECRET} / s3cr3t");
        let out = m.mask(&line);
        assert!(!out.contains("s3cr3t"), "{out}");
        assert_eq!(out, r#"cause: login [redacted "s3cr…" (17 chars)] / [redacted "s…" (6 chars)]"#);
        // Coloured lines are matched through their escapes.
        assert!(!m.mask("\u{1b}[31ms3cr\u{1b}[0m3t").contains("s3cr3t"));
        assert_eq!(Masker::new(None).mask("s3cr3t"), "s3cr3t");
    }
}
