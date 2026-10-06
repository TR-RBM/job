const CATALOGUE: &[(&str, &str)] = &[
    (
        "running legacy Jobs have no boot identity; drain them with the previous version before upgrading",
        "laufende Legacy-Jobs haben keine Boot-Identität; beende sie mit der vorherigen Version vor dem Upgrade",
    ),
    ("invalid signal", "ungültiges Signal"),
    ("job {id} is not running", "Job {id} läuft nicht"),
    (
        "usage: job signal [-s SIGNAL] ID",
        "Aufruf: job signal [-s SIGNAL] ID",
    ),
    (
        "unexpected signal response: {response}",
        "unerwartete Signalantwort: {response}",
    ),
    (
        "signal delivery failed: {error}; some processes may already have received it",
        "Signalzustellung fehlgeschlagen: {error}; einige Prozesse haben das Signal möglicherweise bereits erhalten",
    ),
];

pub fn message(english: &str, values: &[(&str, String)]) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    let template = if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(key, _)| *key == english)
            .map_or(english, |(_, value)| *value)
    } else {
        english
    };
    values
        .iter()
        .fold(template.to_owned(), |text, (key, value)| {
            text.replace(&format!("{{{key}}}"), value)
        })
}
