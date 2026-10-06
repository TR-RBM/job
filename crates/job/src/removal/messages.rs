const CATALOGUE: &[(&str, &str)] = &[
    ("attempt", "Versuch"),
    ("helper", "Helfer"),
    ("no such removal target", "dieses Löschziel existiert nicht"),
    (
        "root Group and default Queue cannot be removed",
        "Root-Group und Standard-Queue können nicht entfernt werden",
    ),
    ("invalid Job record", "ungültiger Job-Datensatz"),
    (
        "invalid workload cgroup state",
        "ungültiger Zustand der Workload-cgroup",
    ),
    (
        "nonempty containers require --recursive; removal never cancels work",
        "nichtleere Sammlungen erfordern --recursive; Entfernen bricht keine Arbeit ab",
    ),
    (
        "only completed Jobs can be removed; cancel separately",
        "du kannst nur abgeschlossene Jobs entfernen; brich Arbeit separat ab",
    ),
    (
        "lost work may remain; removal requires --allow-lost",
        "verlorene Arbeit kann noch laufen; zum Entfernen brauchst du --allow-lost",
    ),
    (
        "previous supervisor has not exited",
        "der bisherige Supervisor läuft noch",
    ),
    (
        "workload cgroup still contains processes",
        "die Workload-cgroup enthält noch Prozesse",
    ),
    (
        "unexpected entry in removal data",
        "unerwarteter Eintrag in den Löschdaten",
    ),
    (
        "removal is not ready",
        "das Entfernen ist noch nicht möglich",
    ),
    ("invalid removal journal", "ungültiges Löschjournal"),
    (
        "removal selection changed",
        "die Auswahl zum Entfernen hat sich geändert",
    ),
    (
        "removal receipt differs from journal",
        "der Löschbeleg weicht vom Journal ab",
    ),
    (
        "network helper has no boot identity",
        "der Netzwerkhelfer hat keine Boot-Identität",
    ),
    (
        "network helper termination is pending",
        "der Netzwerkhelfer wird noch beendet",
    ),
    (
        "a removal transaction is pending; restart the service to reconcile it",
        "eine Löschtransaktion ist offen; starte den Dienst neu, um sie fortzusetzen",
    ),
    (
        "removal committed; cleanup is pending",
        "Löschung beauftragt; Bereinigung steht aus",
    ),
    ("ready", "bereit"),
    ("blocked", "blockiert"),
    (
        "usage: job remove ID [--dry-run] [--allow-lost] [--json]",
        "Aufruf: job remove ID [--dry-run] [--allow-lost] [--json]",
    ),
    (
        "usage: job queue|group remove PATH [--recursive] [--dry-run] [--allow-lost] [--json]",
        "Aufruf: job queue|group remove PFAD [--recursive] [--dry-run] [--allow-lost] [--json]",
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
