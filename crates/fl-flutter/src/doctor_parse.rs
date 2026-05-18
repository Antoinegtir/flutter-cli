//! Parser for `flutter doctor -v` plain-text output.

use fl_core::{DoctorEvent, DoctorStatus};

/// Parse the full stdout of `flutter doctor -v` into a sequence of `DoctorEvent`s,
/// ending with `Done`.
pub fn parse_doctor_output(stdout: &str) -> Vec<DoctorEvent> {
    let mut events = Vec::new();
    let mut current: Option<(DoctorStatus, String, Vec<String>)> = None;

    for raw_line in stdout.lines() {
        if let Some((status, title)) = parse_section_header(raw_line) {
            if let Some((s, t, d)) = current.take() {
                events.push(DoctorEvent::Section { status: s, title: t, details: d });
            }
            current = Some((status, title, Vec::new()));
        } else if let Some((_, _, details)) = current.as_mut() {
            let trimmed = raw_line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("• ") {
                details.push(rest.to_string());
            } else if trimmed.starts_with("✗ ") || trimmed.starts_with("✓ ") || trimmed.starts_with("! ") {
                details.push(trimmed.to_string());
            } else if raw_line.starts_with("    ") || raw_line.starts_with('\t') {
                // Continuation of a previous detail. Append rather than create.
                if let Some(last) = details.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                }
            }
        }
        if raw_line.starts_with("Doctor summary") || raw_line.starts_with("• No issues") {
            break;
        }
    }
    if let Some((s, t, d)) = current.take() {
        events.push(DoctorEvent::Section { status: s, title: t, details: d });
    }
    events.push(DoctorEvent::Done);
    events
}

fn parse_section_header(line: &str) -> Option<(DoctorStatus, String)> {
    let bytes = line.as_bytes();
    if bytes.len() < 4 || bytes[0] != b'[' {
        return None;
    }
    let marker_end = bytes.iter().position(|&b| b == b']')?;
    let marker = &line[1..marker_end];
    let rest = line.get(marker_end + 1..)?.trim();
    if rest.is_empty() {
        return None;
    }
    let status = match marker.trim() {
        "✓" => DoctorStatus::Ok,
        "!" => DoctorStatus::Warning,
        "✗" => DoctorStatus::Error,
        _ => return None,
    };
    Some((status, rest.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_sections_with_details() {
        let input = "\
[✓] Flutter (Channel stable, 3.22.2)
    • Flutter version 3.22.2
    • Engine revision deadbeef
[!] Android Studio (not installed)
    • Try downloading from https://developer.android.com
[✗] Xcode (not installed)

Doctor summary (3 issues found.)
";
        let evs = parse_doctor_output(input);
        // 3 sections + Done = 4
        assert_eq!(evs.len(), 4);
        match &evs[0] {
            DoctorEvent::Section { status, title, details } => {
                assert_eq!(*status, DoctorStatus::Ok);
                assert!(title.contains("Flutter"));
                assert_eq!(details.len(), 2);
            }
            _ => panic!(),
        }
        match &evs[1] {
            DoctorEvent::Section { status, .. } => assert_eq!(*status, DoctorStatus::Warning),
            _ => panic!(),
        }
        match &evs[2] {
            DoctorEvent::Section { status, .. } => assert_eq!(*status, DoctorStatus::Error),
            _ => panic!(),
        }
        assert!(matches!(evs[3], DoctorEvent::Done));
    }

    #[test]
    fn empty_output_emits_only_done() {
        let evs = parse_doctor_output("");
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], DoctorEvent::Done));
    }

    #[test]
    fn ignores_non_section_lines() {
        let input = "Some preamble\n[✓] Flutter\nDoctor summary\n";
        let evs = parse_doctor_output(input);
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0], DoctorEvent::Section { .. }));
        assert!(matches!(evs[1], DoctorEvent::Done));
    }
}
