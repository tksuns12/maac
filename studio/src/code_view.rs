//! Live `.maac` source line highlighting and pure-Rust unified line diffing
//! for GUI-edit previews and AI/external-editor collaboration.

use std::collections::HashSet;

use maac::syntax::Object;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceHighlight {
    pub label: String,
    pub start_line: usize, // 1-based inclusive
    pub end_line: usize,   // 1-based inclusive
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<usize>, // 1-based
    pub new_line: Option<usize>, // 1-based
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiffSummary {
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<DiffHunk>,
}

impl DiffSummary {
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0
    }
}

/// Locate the 1-based inclusive `(start_line, end_line)` of an authored object
/// path (e.g. `["melody_chorus", "f3"]` or `["hall"]`) inside `source`.
pub fn locate_object_lines(source: &str, object_path: &[String]) -> Option<SourceHighlight> {
    if object_path.is_empty() {
        return None;
    }
    let doc = maac::parse(source).ok()?;
    let mut current: &Object = doc.object(&object_path[0])?;
    for seg in &object_path[1..] {
        current = current.child(seg)?;
    }
    let start = current.span.start.min(source.len());
    let end = current.span.end.min(source.len()).max(start);
    let start_line = byte_offset_to_line(source, start);
    let end_offset = if end > start && source.as_bytes().get(end - 1) == Some(&b'\n') {
        end - 1
    } else {
        end
    };
    let end_line = byte_offset_to_line(source, end_offset).max(start_line);
    Some(SourceHighlight {
        label: object_path.join(" / "),
        start_line,
        end_line,
    })
}

/// Return the set of 1-based line numbers in `after` that were added or changed
/// compared to `before` (used to draw green gutter markers in Live Code view).
pub fn changed_new_lines(before: &str, after: &str) -> HashSet<usize> {
    if before == after {
        return HashSet::new();
    }
    let diff = compute_line_diff(before, after, 0);
    let mut set = HashSet::new();
    for hunk in &diff.hunks {
        for line in &hunk.lines {
            if line.kind == DiffLineKind::Added {
                if let Some(n) = line.new_line {
                    set.insert(n);
                }
            }
        }
    }
    set
}

/// Compute a clean unified line diff between `before` and `after` with
/// `context_lines` lines of surrounding context per hunk.
pub fn compute_line_diff(before: &str, after: &str, context_lines: usize) -> DiffSummary {
    if before == after {
        return DiffSummary::default();
    }
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();

    // Trim identical prefix & suffix in O(N) so DP only covers the edited window.
    let mut prefix = 0usize;
    let min_len = a.len().min(b.len());
    while prefix < min_len && a[prefix] == b[prefix] {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix + prefix < min_len && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix] {
        suffix += 1;
    }

    let a_mid = &a[prefix..a.len() - suffix];
    let b_mid = &b[prefix..b.len() - suffix];

    let mut raw: Vec<DiffLine> = Vec::with_capacity(a.len().max(b.len()) + 16);

    for (i, line) in a[..prefix].iter().enumerate() {
        raw.push(DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(i + 1),
            new_line: Some(i + 1),
            text: (*line).to_owned(),
        });
    }

    let mut added = 0usize;
    let mut removed = 0usize;

    // Bounded LCS DP on the middle changed window (fast & exact for typical edits;
    // falls back to single replace block only if a huge rewrite exceeds 400k cells).
    let cells = a_mid.len().saturating_mul(b_mid.len());
    if cells <= 400_000 {
        let m = a_mid.len();
        let n = b_mid.len();
        let stride = n + 1;
        let mut dp = vec![0u32; (m + 1) * stride];
        for i in (0..m).rev() {
            for j in (0..n).rev() {
                dp[i * stride + j] = if a_mid[i] == b_mid[j] {
                    dp[(i + 1) * stride + (j + 1)] + 1
                } else {
                    dp[(i + 1) * stride + j].max(dp[i * stride + (j + 1)])
                };
            }
        }
        let mut i = 0usize;
        let mut j = 0usize;
        while i < m || j < n {
            if i < m && j < n && a_mid[i] == b_mid[j] {
                raw.push(DiffLine {
                    kind: DiffLineKind::Context,
                    old_line: Some(prefix + i + 1),
                    new_line: Some(prefix + j + 1),
                    text: a_mid[i].to_owned(),
                });
                i += 1;
                j += 1;
            } else if i < m && (j == n || dp[(i + 1) * stride + j] >= dp[i * stride + (j + 1)]) {
                raw.push(DiffLine {
                    kind: DiffLineKind::Removed,
                    old_line: Some(prefix + i + 1),
                    new_line: None,
                    text: a_mid[i].to_owned(),
                });
                removed += 1;
                i += 1;
            } else {
                raw.push(DiffLine {
                    kind: DiffLineKind::Added,
                    old_line: None,
                    new_line: Some(prefix + j + 1),
                    text: b_mid[j].to_owned(),
                });
                added += 1;
                j += 1;
            }
        }
    } else {
        for (i, line) in a_mid.iter().enumerate() {
            raw.push(DiffLine {
                kind: DiffLineKind::Removed,
                old_line: Some(prefix + i + 1),
                new_line: None,
                text: (*line).to_owned(),
            });
            removed += 1;
        }
        for (j, line) in b_mid.iter().enumerate() {
            raw.push(DiffLine {
                kind: DiffLineKind::Added,
                old_line: None,
                new_line: Some(prefix + j + 1),
                text: (*line).to_owned(),
            });
            added += 1;
        }
    }

    let a_tail = a.len() - suffix;
    let b_tail = b.len() - suffix;
    for k in 0..suffix {
        raw.push(DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(a_tail + k + 1),
            new_line: Some(b_tail + k + 1),
            text: a[a_tail + k].to_owned(),
        });
    }

    // Group changed indices into hunks with `context_lines` around each change.
    let change_indices: Vec<usize> = raw
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| (line.kind != DiffLineKind::Context).then_some(idx))
        .collect();

    let mut hunks = Vec::new();
    let mut cur_range: Option<(usize, usize)> = None;

    for idx in change_indices {
        let lo = idx.saturating_sub(context_lines);
        let hi = (idx + context_lines).min(raw.len().saturating_sub(1));
        match cur_range {
            Some((r_lo, r_hi)) if lo <= r_hi + 1 => {
                cur_range = Some((r_lo, hi.max(r_hi)));
            }
            Some((r_lo, r_hi)) => {
                hunks.push(build_hunk(&raw[r_lo..=r_hi]));
                cur_range = Some((lo, hi));
            }
            None => {
                cur_range = Some((lo, hi));
            }
        }
    }
    if let Some((r_lo, r_hi)) = cur_range {
        hunks.push(build_hunk(&raw[r_lo..=r_hi]));
    }

    DiffSummary {
        added,
        removed,
        hunks,
    }
}

fn build_hunk(slice: &[DiffLine]) -> DiffHunk {
    let old_start = slice.iter().find_map(|l| l.old_line).unwrap_or(1);
    let new_start = slice.iter().find_map(|l| l.new_line).unwrap_or(1);
    let old_count = slice
        .iter()
        .filter(|l| l.kind != DiffLineKind::Added)
        .count();
    let new_count = slice
        .iter()
        .filter(|l| l.kind != DiffLineKind::Removed)
        .count();
    DiffHunk {
        header: format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"),
        lines: slice.to_vec(),
    }
}

fn byte_offset_to_line(source: &str, offset: usize) -> usize {
    let clamped = offset.min(source.len());
    source[..clamped].bytes().filter(|&b| b == b'\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{apply_note_edit, NoteEdit};
    use crate::project::load_and_compile;
    use std::path::Path;

    #[test]
    fn locates_note_and_node_lines_in_ferry() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry");
        let src = &data.bundle.sources[&data.bundle.entry];

        let note_hl = locate_object_lines(src, &["melody_chorus".into(), "f3".into()])
            .expect("locate melody_chorus/f3");
        let line_text = src.lines().nth(note_hl.start_line - 1).unwrap();
        assert!(
            line_text.contains("note f3") && line_text.contains("B5"),
            "unexpected line {}: {line_text}",
            note_hl.start_line
        );

        let node_hl = locate_object_lines(src, &["hall".into()]).expect("locate node hall");
        let hall_line = src.lines().nth(node_hl.start_line - 1).unwrap();
        assert!(
            hall_line.contains("node hall"),
            "unexpected hall line {}: {hall_line}",
            node_hl.start_line
        );
    }

    #[test]
    fn computes_minimal_diff_after_note_edit() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/ferry/ferry.maac");
        let data = load_and_compile(&path).expect("compile ferry");
        let before = &data.bundle.sources[&data.bundle.entry];
        let (edited, _) = apply_note_edit(
            &data.bundle,
            &["melody_chorus".into(), "f3".into()],
            NoteEdit::SetPitchMidi(71),
            &data.limits,
        )
        .expect("transpose f3 to B4");
        let after = &edited.sources[&edited.entry];

        let diff = compute_line_diff(before, after, 2);
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        assert_eq!(diff.hunks.len(), 1);
        assert!(diff.hunks[0]
            .lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Removed && l.text.contains("pitch = B5")));
        assert!(diff.hunks[0]
            .lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Added && l.text.contains("pitch = B4")));
    }
}
