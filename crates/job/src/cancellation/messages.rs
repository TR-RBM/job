const CATALOGUE: &[(&str, &str)] = &[
    ("invalid cancellation journal", "ungültiges Abbruchjournal"),
    ("invalid Job record", "ungültiger Job-Datensatz"),
    (
        "collection cancellation requires --recursive",
        "zum Abbrechen einer Sammlung brauchst du --recursive",
    ),
    (
        "no such cancellation target",
        "dieses Abbruchziel existiert nicht",
    ),
    (
        "Job has already finished",
        "der Job ist bereits abgeschlossen",
    ),
    (
        "cancellation selection changed",
        "die Auswahl zum Abbrechen hat sich geändert",
    ),
    ("cancellation is pending", "Abbruch steht aus"),
    ("cancellation recorded", "Abbruch gespeichert"),
    ("cancelled before it started", "vor dem Start abgebrochen"),
    (
        "stopped: cancellation requested",
        "gestoppt: Abbruch angefordert",
    ),
    (
        "selected attempt is no longer current",
        "der ausgewählte Versuch ist nicht mehr aktuell",
    ),
    (
        "usage: job queue|group cancel --recursive PATH [--dry-run] [--json]",
        "Aufruf: job queue|group cancel --recursive PFAD [--dry-run] [--json]",
    ),
];

pub fn message(english: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(key, _)| *key == english)
            .map_or(english, |(_, value)| *value)
            .to_owned()
    } else {
        english.to_owned()
    }
}
