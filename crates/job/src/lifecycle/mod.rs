const CATALOGUE: &[(&str, &str)] = &[
    ("invalid attempt archive", "ungültiges Versuchsarchiv"),
    ("no such Job", "dieser Job existiert nicht"),
    ("no such attempt", "dieser Versuch existiert nicht"),
    (
        "retry requires the expected completed attempt",
        "die Wiederholung erfordert den erwarteten abgeschlossenen Versuch",
    ),
    (
        "lost work may still exist; retry requires --allow-lost",
        "verlorene Arbeit läuft möglicherweise noch; verwende --allow-lost für die Wiederholung",
    ),
    (
        "previous workload cgroup is still populated",
        "in der bisherigen Workload-cgroup laufen noch Prozesse",
    ),
    (
        "saved environment is unavailable; use --current-env to replace it",
        "die gespeicherte Umgebung ist nicht verfügbar; ersetze sie mit --current-env",
    ),
    (
        "original Queue was removed; choose --queue PATH",
        "die ursprüngliche Queue wurde entfernt; wähle --queue PFAD",
    ),
    (
        "attempt counter exhausted",
        "der Versuchszähler ist ausgeschöpft",
    ),
    (
        "previous supervisor has not exited; retry was not created",
        "der bisherige Supervisor läuft noch; es wurde keine Wiederholung erstellt",
    ),
    (
        "usage: job attempts ID [--json]",
        "Aufruf: job attempts ID [--json]",
    ),
    (
        "usage: job retry ID [--hold] [--current-env] [--queue PATH] [--allow-lost] [--json]",
        "Aufruf: job retry ID [--hold] [--current-env] [--queue PFAD] [--allow-lost] [--json]",
    ),
    ("usage: job release ID", "Aufruf: job release ID"),
    (
        "only held or queued Jobs can be edited",
        "du kannst nur gehaltene oder wartende Jobs bearbeiten",
    ),
    (
        "only held Jobs can be released",
        "du kannst nur gehaltene Jobs freigeben",
    ),
    (
        "unexpected file in an editable Job",
        "unerwartete Datei in einem bearbeitbaren Job",
    ),
    ("held", "gehalten"),
    ("starting", "wird gestartet"),
    ("stopping", "wird beendet"),
    ("suspended", "angehalten"),
    (
        "queue is closed to release",
        "die Queue ist für Freigaben geschlossen",
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
