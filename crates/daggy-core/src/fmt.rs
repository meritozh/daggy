//! Human-readable byte formatting (binary units, DaisyDisk-style).

/// Format a byte count using binary units (KiB, MiB, GiB, …) with one decimal
/// place, e.g. `1.2 GB`, `800 MB`, `12 KB`, `340 B`.
///
/// We use the conventional 1024-based divisor but the short SI-style suffixes
/// (KB/MB/GB), matching what users expect from disk tools.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB", "EB"];
    if bytes < 1024 {
        return format!("{} {}", bytes, UNITS[0]);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    // Drop the decimal for values >= 100 to keep it compact.
    if value >= 100.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// Format a count of items, e.g. `12,401 items`.
pub fn human_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(human_bytes(819u64 * 1024 * 1024), "819 MB");
        assert_eq!(human_bytes(1288490188800u64), "1.2 TB");
    }

    #[test]
    fn formats_counts() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(999), "999");
        assert_eq!(human_count(12401), "12,401");
        assert_eq!(human_count(1000000), "1,000,000");
    }
}
