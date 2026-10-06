const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;
const TIB: u64 = 1024 * GIB;

pub fn format_bytes(bytes: u64) -> String {
    let scaled = |unit: u64| bytes as f64 / unit as f64;
    if bytes >= TIB {
        format!("{:.2} TiB", scaled(TIB))
    } else if bytes >= GIB {
        format!("{:.2} GiB", scaled(GIB))
    } else if bytes >= MIB {
        format!("{:.0} MiB", scaled(MIB))
    } else if bytes >= KIB {
        format!("{:.0} KiB", scaled(KIB))
    } else {
        format!("{bytes} B")
    }
}

pub fn format_duration_ms(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds == 0 {
        return format!("{ms}ms");
    }
    let (hours, minutes, secs) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{secs:02}s")
    } else {
        format!("{secs}s")
    }
}

pub fn parse_bytes(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let unit_bytes = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => KIB,
        "m" | "mb" | "mib" => MIB,
        "g" | "gb" | "gib" => GIB,
        "t" | "tb" | "tib" => TIB,
        other => {
            return Err(format!(
                "unknown size unit `{other}` in `{text}`; use K, M, G or T"
            ));
        }
    };
    let value: f64 = number
        .parse()
        .map_err(|_| format!("`{text}` is not a size such as 512M or 4G"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("`{text}` is not a size such as 512M or 4G"));
    }
    Ok((value * unit_bytes as f64).round() as u64)
}

pub fn parse_duration_ms(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let unit_ms = match unit {
        "ms" => 1.0,
        "" | "s" => 1000.0,
        "m" | "min" => 60_000.0,
        "h" => 3_600_000.0,
        other => {
            return Err(format!(
                "unknown time unit `{other}` in `{text}`; use ms, s, m or h"
            ));
        }
    };
    let value: f64 = number
        .parse()
        .map_err(|_| format!("`{text}` is not a duration such as 90s or 10m"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("`{text}` is not a duration such as 90s or 10m"));
    }
    Ok((value * unit_ms).round() as u64)
}

pub fn parse_rate(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let unit = unit.trim().trim_end_matches("/s");
    let bits_per_unit: u64 = match unit {
        "bit" => 1,
        "kbit" | "Kbit" => 1_000,
        "Mbit" => 1_000_000,
        "Gbit" => 1_000_000_000,
        "B" => 8,
        "kB" | "KB" => 8_000,
        "MB" => 8_000_000,
        "GB" => 8_000_000_000,
        "KiB" => 8 * 1024,
        "MiB" => 8 * 1024 * 1024,
        "GiB" => 8 * 1024 * 1024 * 1024,
        _ => {
            return Err(format!(
                "`{text}` is not a rate; write bits as 10Mbit or bytes as 10MB/s, since a bare 10M is bits to tc and bytes to curl"
            ));
        }
    };
    let value: f64 = number
        .parse()
        .map_err(|_| format!("`{text}` is not a rate such as 10Mbit or 2MB/s"))?;
    let bits = (value * bits_per_unit as f64).round();
    if !bits.is_finite() || bits < 8_000.0 {
        return Err(format!(
            "`{text}` is below 8 kbit/s, the least job can shape"
        ));
    }
    Ok(bits as u64)
}

pub fn format_rate(bits_per_second: u64) -> String {
    let (value, unit) = if bits_per_second >= 1_000_000_000 {
        (bits_per_second as f64 / 1e9, "Gbit/s")
    } else if bits_per_second >= 1_000_000 {
        (bits_per_second as f64 / 1e6, "Mbit/s")
    } else {
        (bits_per_second as f64 / 1e3, "kbit/s")
    };
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_shown_in_the_largest_whole_unit() {
        assert_eq!(format_bytes(4 * GIB), "4.00 GiB");
        assert_eq!(format_bytes(300 * MIB), "300 MiB");
        assert_eq!(format_bytes(12), "12 B");
    }

    #[test]
    fn durations_read_as_minutes_and_seconds() {
        assert_eq!(format_duration_ms(192_000), "3m12s");
        assert_eq!(format_duration_ms(41_500), "41s");
        assert_eq!(format_duration_ms(850), "850ms");
        assert_eq!(format_duration_ms(3_720_000), "1h02m");
    }

    #[test]
    fn sizes_parse_with_binary_units() {
        assert_eq!(parse_bytes("512M"), Ok(512 * MIB));
        assert_eq!(parse_bytes("1.5G"), Ok(3 * GIB / 2));
        assert_eq!(parse_bytes("100"), Ok(100));
        assert!(parse_bytes("4X").is_err());
    }

    #[test]
    fn durations_parse_with_units() {
        assert_eq!(parse_duration_ms("90s"), Ok(90_000));
        assert_eq!(parse_duration_ms("10m"), Ok(600_000));
        assert_eq!(parse_duration_ms("250ms"), Ok(250));
        assert!(parse_duration_ms("-1s").is_err());
    }

    #[test]
    fn a_rate_names_bits_or_bytes_and_a_bare_prefix_is_refused() {
        assert_eq!(parse_rate("10Mbit"), Ok(10_000_000));
        assert_eq!(parse_rate("10Mbit/s"), Ok(10_000_000));
        assert_eq!(parse_rate("2MB/s"), Ok(16_000_000));
        assert_eq!(parse_rate("1MiB/s"), Ok(8_388_608));
        assert_eq!(parse_rate("500kbit"), Ok(500_000));
        assert!(parse_rate("10M").is_err());
        assert!(parse_rate("10").is_err());
        assert!(parse_rate("1kbit").is_err());
    }

    #[test]
    fn a_rate_is_shown_in_bits() {
        assert_eq!(format_rate(10_000_000), "10 Mbit/s");
        assert_eq!(format_rate(16_000_000), "16 Mbit/s");
        assert_eq!(format_rate(2_500_000), "2.5 Mbit/s");
        assert_eq!(format_rate(500_000), "500 kbit/s");
    }
}
