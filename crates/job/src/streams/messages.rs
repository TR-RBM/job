const CATALOGUE: &[(&str, &str)] = &[
    (
        "output wait timed out; the Job continues",
        "Wartezeit auf Ausgabe abgelaufen; der Job läuft weiter",
    ),
    (
        "timestamps are unavailable for this recording",
        "Zeitstempel sind für diese Aufzeichnung nicht verfügbar",
    ),
    (
        "usage: job logs ID [-f|--follow] [--stream all|stdout|stderr|combined|diagnostic] [--attempt N] [--raw|--json] [--since-ms N] [--until-ms N] [--grep TEXT]",
        "Aufruf: job logs ID [-f|--follow] [--stream all|stdout|stderr|combined|diagnostic] [--attempt N] [--raw|--json] [--since-ms N] [--until-ms N] [--grep TEXT]",
    ),
    ("invalid output recording", "ungültige Ausgabeaufzeichnung"),
    (
        "output recording is incomplete",
        "Ausgabeaufzeichnung ist unvollständig",
    ),
    (
        "output pipes remained open after workload exit",
        "Ausgabepipes blieben nach dem Prozessende offen",
    ),
    (
        "the record of Job {id} cannot be read; its output is counted in the output budget and is never trimmed",
        "der Datensatz von Job {id} lässt sich nicht lesen; seine Ausgabe zählt zum Ausgabebudget und wird nie gekürzt",
    ),
    (
        "output budget: the records of Jobs {ids} cannot be read; their output is counted and never trimmed",
        "Ausgabebudget: die Datensätze der Jobs {ids} lassen sich nicht lesen; ihre Ausgabe zählt mit und wird nie gekürzt",
    ),
    (
        "output records were removed by retention",
        "Ausgabeeinträge wurden durch die Aufbewahrungsgrenze entfernt",
    ),
    (
        "separate streams are unavailable for this recording",
        "getrennte Streams sind für diese Aufzeichnung nicht verfügbar",
    ),
    (
        "update the service before requesting live output",
        "aktualisiere den Dienst, bevor du Live-Ausgabe anforderst",
    ),
    (
        "output client disconnected; the Job continues",
        "Ausgabeclient getrennt; der Job läuft weiter",
    ),
    (
        "an output quota must be between {minimum} and {maximum}",
        "ein Ausgabekontingent muss zwischen {minimum} und {maximum} liegen",
    ),
    ("this Job", "dieser Job"),
    ("service configuration", "Dienstkonfiguration"),
    ("built-in", "eingebaut"),
    (
        "kept per stream: at least the first {head} and the last {tail}",
        "pro Stream behalten: mindestens die ersten {head} und die letzten {tail}",
    ),
    (
        "kept for all streams together: the first {head} and the last {tail}",
        "für alle Streams zusammen behalten: die ersten {head} und die letzten {tail}",
    ),
    ("{stream} {bytes} bytes", "{stream} {bytes} Bytes"),
    (
        "output quota per stream: first {head} ({head_source}), last {tail} ({tail_source})",
        "Ausgabekontingent pro Stream: die ersten {head} ({head_source}), die letzten {tail} ({tail_source})",
    ),
    (
        "output quota for all streams together: first {head}, last {tail} (built-in)",
        "Ausgabekontingent für alle Streams zusammen: die ersten {head}, die letzten {tail} (eingebaut)",
    ),
    (
        "removed by retention: {streams}",
        "durch die Aufbewahrungsgrenze entfernt: {streams}",
    ),
    (
        "{bytes} bytes were dropped before they reached this recording",
        "{bytes} Bytes gingen verloren, bevor sie diese Aufzeichnung erreichten",
    ),
    (
        "the recording was removed by the service output budget ({bytes} bytes)",
        "die Aufzeichnung wurde durch das Ausgabebudget des Dienstes entfernt ({bytes} Bytes)",
    ),
    (
        "the output of this attempt was removed to keep the service within its output budget",
        "die Ausgabe dieses Versuchs wurde entfernt, damit der Dienst in seinem Ausgabebudget bleibt",
    ),
    (
        "Output quota: --output-head SIZE; --output-tail SIZE",
        "Ausgabekontingent: --output-head SIZE; --output-tail SIZE",
    ),
    (
        "Each recorded stream keeps at least its first and its last SIZE, between 1M and 1G; what lies between is removed and reported. Queue and Group defaults use the --job- prefix; the service configuration sets [output] head_bytes, tail_bytes and budget_bytes. There is no unlimited.",
        "Jeder aufgezeichnete Stream behält mindestens seine ersten und seine letzten SIZE, zwischen 1M und 1G; was dazwischen liegt, wird entfernt und gemeldet. Standardwerte für Queues und Gruppen verwenden das Präfix --job-; die Dienstkonfiguration setzt [output] head_bytes, tail_bytes und budget_bytes. Ein unlimited gibt es nicht.",
    ),
    (
        "duplicate output quota",
        "doppelte Ausgabekontingent-Option",
    ),
];
pub fn message(key: &str) -> String {
    text(key, &[])
}

pub fn text(key: &str, values: &[(&str, String)]) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    let template = if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(en, _)| *en == key)
            .map_or(key, |(_, de)| *de)
    } else {
        key
    };
    values
        .iter()
        .fold(template.to_owned(), |text, (name, value)| {
            text.replace(&format!("{{{name}}}"), value)
        })
}
