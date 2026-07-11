//! Pure text helpers for the vasm driver (§4.1/§4.2 rules): org injection,
//! org/rorg-conflict detection, source bound, diagnostics
//! parse/sanitize/bound. No I/O, no subprocess — fully unit-tested without
//! vasm.

use super::SRC_MAX_BYTES;
use crate::types::Addr;

pub const DIAG_MAX: usize = 16;
pub const DIAG_LINE_MAX: usize = 200;
pub const LISTING_MAX_BYTES: usize = 4096;

/// §4.1: size-check, org-conflict-check, org injection. Err is a human
/// message the tool maps to invalid_params.
pub fn prepare_source(source: &str, org: Option<Addr>) -> std::result::Result<String, String> {
    if source.len() > SRC_MAX_BYTES {
        return Err(format!("source exceeds {SRC_MAX_BYTES} bytes"));
    }
    if org.is_some() && has_org_directive(source) {
        return Err(
            "source must not contain org/rorg when org/addr is supplied (vasm -Fbin \
             zero-pads between sections)"
                .to_string(),
        );
    }
    Ok(match org {
        Some(a) => format!("\torg ${:x}\n{source}", a.raw()),
        None => source.to_string(),
    })
}

/// §4.1 pinned tokenizer: any line whose first token (after stripping `;`
/// comments, column-0 `*` comment lines, and one optional `label:` token)
/// is `org`/`rorg`, case-insensitive.
pub fn has_org_directive(source: &str) -> bool {
    source.lines().any(is_org_line)
}

fn is_org_line(line: &str) -> bool {
    if line.starts_with('*') {
        return false;
    }
    let without_comment = line.split(';').next().unwrap_or("");
    let mut tokens = without_comment.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    let directive = if let Some(label) = first.strip_suffix(':') {
        let _ = label; // label text itself is irrelevant, only its presence
        match tokens.next() {
            Some(t) => t,
            None => return false,
        }
    } else {
        first
    };
    directive.eq_ignore_ascii_case("org") || directive.eq_ignore_ascii_case("rorg")
}

/// §0 shapes: lines starting with "error" / "fatal error" → errors; lines
/// starting with "warning" → warnings. Each kept line is sanitized (every
/// occurrence of `src_name`, e.g. "asm-1234-7.s", replaced by "source") and
/// truncated to DIAG_LINE_MAX chars; each list capped at DIAG_MAX with a
/// final "… (N more)" entry when truncated. `>`-echo lines are dropped (the
/// error already carries the line number).
pub fn parse_diagnostics(diag_text: &str, src_name: &str) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for line in diag_text.lines() {
        if line.starts_with('>') {
            continue;
        }
        if line.starts_with("error") || line.starts_with("fatal error") {
            errors.push(sanitize_and_truncate(line, src_name));
        } else if line.starts_with("warning") {
            warnings.push(sanitize_and_truncate(line, src_name));
        }
    }
    (bound_list(errors), bound_list(warnings))
}

fn sanitize_and_truncate(line: &str, src_name: &str) -> String {
    let sanitized = if src_name.is_empty() {
        line.to_string()
    } else {
        line.replace(src_name, "source")
    };
    sanitized.chars().take(DIAG_LINE_MAX).collect()
}

fn bound_list(mut v: Vec<String>) -> Vec<String> {
    if v.len() > DIAG_MAX {
        let more = v.len() - DIAG_MAX;
        v.truncate(DIAG_MAX);
        v.push(format!("… ({more} more)"));
    }
    v
}

/// Truncate to LISTING_MAX_BYTES on a char boundary, appending
/// "\n… (listing truncated)" when cut; sanitize `src_name` → "source".
pub fn bound_listing(text: &str, src_name: &str) -> String {
    let sanitized = if src_name.is_empty() {
        text.to_string()
    } else {
        text.replace(src_name, "source")
    };
    if sanitized.len() <= LISTING_MAX_BYTES {
        return sanitized;
    }
    let mut cut = LISTING_MAX_BYTES;
    while !sanitized.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n… (listing truncated)", &sanitized[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_injects_org() {
        let out = prepare_source("\tnop\n", Some(Addr::new(0x70000))).unwrap();
        assert_eq!(out, "\torg $70000\n\tnop\n");
    }

    #[test]
    fn prepare_no_org_passthrough() {
        let out = prepare_source("\tnop\n", None).unwrap();
        assert_eq!(out, "\tnop\n");
    }

    #[test]
    fn prepare_rejects_oversize() {
        let src = "a".repeat(SRC_MAX_BYTES + 1);
        let err = prepare_source(&src, None).unwrap_err();
        assert!(err.contains("65536"));
    }

    #[test]
    fn prepare_rejects_org_conflict() {
        let err = prepare_source("\torg $100\n\tnop\n", Some(Addr::new(0x70000))).unwrap_err();
        assert!(err.contains("org/rorg"));
    }

    #[test]
    fn has_org_variants() {
        assert!(has_org_directive("  ORG $100"));
        assert!(has_org_directive("lbl:\trorg 4"));
        assert!(!has_org_directive("\tnop ; org here"));
        assert!(!has_org_directive("* org comment"));
        assert!(!has_org_directive("move.w #1,org_flag"));
    }

    #[test]
    fn diag_error_shapes() {
        let diag = "error 3007: undefined symbol <bogus_label>\n\
                     error 9 in line 3 of \"asm-1-1.s\": instruction not supported on \
                     selected architecture\n\
                     >\tmove.w #$777,bogus_label\n";
        let (errors, warnings) = parse_diagnostics(diag, "asm-1-1.s");
        assert_eq!(errors.len(), 2);
        assert!(errors[1].contains("\"source\""));
        assert!(warnings.is_empty());
    }

    #[test]
    fn diag_warning_shape() {
        let diag = "warning 51 in line 2 of \"asm-1-1.s\": instruction has been \
                     auto-aligned\n>\tnop\n";
        let (errors, warnings) = parse_diagnostics(diag, "asm-1-1.s");
        assert!(errors.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("\"source\""));
    }

    #[test]
    fn diag_fatal_shape() {
        let diag = "fatal error 14: could not open </x> for output\naborting...\n";
        let (errors, _) = parse_diagnostics(diag, "asm-1-1.s");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("fatal error 14"));
    }

    #[test]
    fn diag_caps() {
        let diag = (0..40)
            .map(|i| format!("error {i}: x"))
            .collect::<Vec<_>>()
            .join("\n");
        let (errors, _) = parse_diagnostics(&diag, "x");
        assert_eq!(errors.len(), 17);
        assert_eq!(errors[16], "… (24 more)");
    }

    #[test]
    fn diag_line_truncated() {
        let long = format!("error {}", "x".repeat(500));
        let (errors, _) = parse_diagnostics(&long, "none");
        assert!(errors[0].chars().count() <= DIAG_LINE_MAX);
    }

    #[test]
    fn listing_bounded() {
        let text = format!("hello asm-1-1.s {}", "y".repeat(10 * 1024));
        let out = bound_listing(&text, "asm-1-1.s");
        assert!(out.len() <= LISTING_MAX_BYTES + 64);
        assert!(out.contains("source"));
        assert!(out.contains("truncated"));
    }
}
