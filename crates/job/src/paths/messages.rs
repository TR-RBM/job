const CATALOGUE: &[(&str, &str)] = &[
    (
        "cannot locate the configuration: set HOME, XDG_CONFIG_HOME or JOB_CONFIG",
        "die Konfiguration ist nicht auffindbar: setze HOME, XDG_CONFIG_HOME oder JOB_CONFIG",
    ),
    (
        "cannot locate the state directory: set HOME, XDG_STATE_HOME or JOB_STATE_DIR",
        "das Zustandsverzeichnis ist nicht auffindbar: setze HOME, XDG_STATE_HOME oder JOB_STATE_DIR",
    ),
    (
        "cannot locate the cache directory: set HOME, XDG_CACHE_HOME or JOB_CACHE_DIR",
        "das Cache-Verzeichnis ist nicht auffindbar: setze HOME, XDG_CACHE_HOME oder JOB_CACHE_DIR",
    ),
    (
        "cannot locate the service socket",
        "der Socket des Dienstes ist nicht auffindbar",
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
