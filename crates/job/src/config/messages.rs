const CATALOGUE: &[(&str, &str)] = &[
    (
        "unsupported configuration schema {version}",
        "nicht unterstützte Konfigurationsversion {version}",
    ),
    (
        "configuration valid: {path}",
        "Konfiguration gültig: {path}",
    ),
    (
        "profile must be ordinary or legacy",
        "wähle als Profil ordinary oder legacy",
    ),
    (
        "created {path}; set JOB_CONFIG to this file when starting the daemon",
        "{path} erstellt; setze JOB_CONFIG beim Start des Daemons auf diese Datei",
    ),
    (
        "unexpected configuration response {response}",
        "unerwartete Konfigurationsantwort {response}",
    ),
    (
        "usage: job config check FILE | init --profile ordinary|legacy FILE | show [--json] | reload",
        "Aufruf: job config check DATEI | init --profile ordinary|legacy DATEI | show [--json] | reload",
    ),
    (
        "active Jobs use a different service policy; restart with their original profile and drain before changing it",
        "aktive Jobs verwenden ein anderes Dienstprofil; starte mit ihrem ursprünglichen Profil und beende sie vor dem Profilwechsel",
    ),
    (
        "legacy state requires an explicit service profile; create a legacy configuration and set JOB_CONFIG before starting this daemon",
        "vorhandener Legacy-Zustand benötigt ein explizites Dienstprofil; erstelle eine Legacy-Konfiguration und setze JOB_CONFIG vor dem Start",
    ),
    (
        "configuration reload requires draining active and waiting Jobs",
        "beende aktive und wartende Jobs, bevor du die Konfiguration neu lädst",
    ),
    (
        "changing service profile requires a daemon restart after draining",
        "beende aktive und wartende Jobs und starte den Daemon neu, um das Dienstprofil zu wechseln",
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
