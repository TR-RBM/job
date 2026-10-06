const CATALOGUE: &[(&str, &str)] = &[
    ("its turn to start", "seinen Platz in der Startreihenfolge"),
    (
        "a sync thread ended early",
        "ein Thread zum Sichern endete vorzeitig",
    ),
    (
        "cancellation could not be completed and is tried again; operation",
        "ein Abbruch ließ sich nicht abschließen und wird erneut versucht; Vorgang",
    ),
    (
        "the starter failed in one pass and goes on; further failures are only counted",
        "der Starter ist in einem Durchlauf fehlgeschlagen und läuft weiter; weitere Fehler werden nur gezählt",
    ),
    (
        "cannot make the cancelled records durable",
        "die abgebrochenen Einträge lassen sich nicht dauerhaft sichern",
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
