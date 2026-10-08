const CATALOGUE: &[(&str, &str)] = &[
    (
        "metrics listen must be an IP address and a port, such as 127.0.0.1:9877 or [::1]:9877, not `{value}`",
        "metrics listen muss eine IP-Adresse mit Port sein, etwa 127.0.0.1:9877 oder [::1]:9877, nicht `{value}`",
    ),
    (
        "cannot listen for metrics on {address}: {error}",
        "auf {address} können keine Metriken angeboten werden: {error}",
    ),
    (
        "metrics at http://{address}/metrics",
        "Metriken unter http://{address}/metrics",
    ),
    (
        "the metrics address {address} is not a loopback address; everyone who can reach it can read the metrics",
        "die Metrik-Adresse {address} ist keine Loopback-Adresse; alle, die sie erreichen, können die Metriken lesen",
    ),
    (
        "job metrics takes no `{word}`; see job metrics --help",
        "job metrics nimmt kein `{word}`; siehe job metrics --help",
    ),
    (
        "the service gave an unexpected answer",
        "der Dienst hat unerwartet geantwortet",
    ),
    (
        "changing the metrics listener requires a daemon restart",
        "starte den Daemon neu, um die Metrik-Adresse zu ändern",
    ),
];

pub fn message(key: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(english, _)| *english == key)
            .map_or(key, |(_, german)| *german)
            .to_owned()
    } else {
        key.to_owned()
    }
}
